use std::future::Future;
use std::io;
use std::net::{
    Ipv4Addr, SocketAddr, TcpListener as StdTcpListener, TcpStream as StdTcpStream,
    UdpSocket as StdUdpSocket,
};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use cheetah_codec::MonoTime;
use futures::channel::oneshot;
use thiserror::Error;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SpawnError {
    #[error("local task context is required")]
    LocalContextRequired,
    #[error("{0}")]
    RuntimeUnavailable(String),
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct TaskJoinError {
    message: String,
}

impl TaskJoinError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("oneshot sender dropped before completion")]
pub struct OneShotRecvError;

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("oneshot receiver dropped before completion")]
pub struct OneShotSendError;

#[derive(Debug)]
pub struct OneShotSender {
    inner: oneshot::Sender<()>,
}

impl OneShotSender {
    pub fn send(self) -> Result<(), OneShotSendError> {
        self.inner.send(()).map_err(|_| OneShotSendError)
    }
}

#[derive(Debug)]
pub struct OneShotReceiver {
    inner: oneshot::Receiver<()>,
}

impl OneShotReceiver {
    pub async fn recv(&mut self) -> Result<(), OneShotRecvError> {
        Pin::new(self).await
    }
}

impl Future for OneShotReceiver {
    type Output = Result<(), OneShotRecvError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.inner).poll(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(_)) => Poll::Ready(Err(OneShotRecvError)),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub fn oneshot_channel() -> (OneShotSender, OneShotReceiver) {
    let (tx, rx) = oneshot::channel();
    (OneShotSender { inner: tx }, OneShotReceiver { inner: rx })
}

#[derive(Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    waiters: Mutex<Vec<Option<Waker>>>,
    children: Mutex<Vec<Weak<CancellationState>>>,
}

impl CancellationState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            cancelled: AtomicBool::new(false),
            waiters: Mutex::new(Vec::new()),
            children: Mutex::new(Vec::new()),
        })
    }

    fn lock_waiters(&self) -> std::sync::MutexGuard<'_, Vec<Option<Waker>>> {
        self.waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_children(&self) -> std::sync::MutexGuard<'_, Vec<Weak<CancellationState>>> {
        self.children
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn cancel_state(root: &Arc<CancellationState>) {
    if root.cancelled.swap(true, Ordering::AcqRel) {
        return;
    }

    let waiters = {
        let mut guard = root.lock_waiters();
        std::mem::take(&mut *guard)
    };
    for waiter in waiters.into_iter().flatten() {
        waiter.wake();
    }

    let children = {
        let mut guard = root.lock_children();
        std::mem::take(&mut *guard)
    };
    for child in children {
        if let Some(child) = child.upgrade() {
            cancel_state(&child);
        }
    }
}

#[derive(Debug, Clone)]
pub struct CancellationToken {
    inner: Arc<CancellationState>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            inner: CancellationState::new(),
        }
    }

    pub fn child_token(&self) -> Self {
        let child = Self::new();
        if self.is_cancelled() {
            child.cancel();
            return child;
        }

        {
            let mut guard = self.inner.lock_children();
            guard.retain(|entry| entry.upgrade().is_some());
            guard.push(Arc::downgrade(&child.inner));
        }
        if self.is_cancelled() {
            child.cancel();
        }
        child
    }

    pub fn cancel(&self) {
        cancel_state(&self.inner);
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    pub fn cancelled(&self) -> CancellationFuture {
        CancellationFuture {
            inner: self.inner.clone(),
            waiter_slot: None,
        }
    }
}

pub struct CancellationFuture {
    inner: Arc<CancellationState>,
    waiter_slot: Option<usize>,
}

impl CancellationFuture {
    fn deregister(&mut self) {
        let Some(slot) = self.waiter_slot.take() else {
            return;
        };
        let mut waiters = self.inner.lock_waiters();
        if let Some(entry) = waiters.get_mut(slot) {
            *entry = None;
        }
        while waiters.last().is_some_and(Option::is_none) {
            let _ = waiters.pop();
        }
    }
}

impl Future for CancellationFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.inner.cancelled.load(Ordering::Acquire) {
            self.deregister();
            return Poll::Ready(());
        }
        match self.waiter_slot {
            Some(slot) => {
                let mut clear_slot = false;
                let mut waiters = self.inner.lock_waiters();
                if let Some(entry) = waiters.get_mut(slot) {
                    if entry
                        .as_ref()
                        .is_none_or(|current| !current.will_wake(cx.waker()))
                    {
                        *entry = Some(cx.waker().clone());
                    }
                } else {
                    clear_slot = true;
                }
                drop(waiters);
                if clear_slot {
                    self.waiter_slot = None;
                }
            }
            None => {
                let slot = {
                    let mut waiters = self.inner.lock_waiters();
                    let slot = waiters.iter().position(Option::is_none).unwrap_or_else(|| {
                        waiters.push(None);
                        waiters.len() - 1
                    });
                    waiters[slot] = Some(cx.waker().clone());
                    slot
                };
                self.waiter_slot = Some(slot);
            }
        }
        if self.inner.cancelled.load(Ordering::Acquire) {
            self.deregister();
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Drop for CancellationFuture {
    fn drop(&mut self) {
        self.deregister();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpRecvMeta {
    pub from: SocketAddr,
    pub len: usize,
}

#[async_trait]
pub trait AsyncUdpSocket: Send + Sync {
    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<UdpRecvMeta>;
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize>;
    fn local_addr(&self) -> io::Result<SocketAddr>;
    fn join_multicast_v4(&self, _multiaddr: Ipv4Addr, _interface: Ipv4Addr) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "join_multicast_v4 is not supported by this runtime socket",
        ))
    }
    fn leave_multicast_v4(&self, _multiaddr: Ipv4Addr, _interface: Ipv4Addr) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "leave_multicast_v4 is not supported by this runtime socket",
        ))
    }
    fn set_multicast_ttl_v4(&self, _ttl: u32) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "set_multicast_ttl_v4 is not supported by this runtime socket",
        ))
    }
    fn set_multicast_loop_v4(&self, _enabled: bool) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "set_multicast_loop_v4 is not supported by this runtime socket",
        ))
    }
}

#[async_trait]
pub trait AsyncTcpStream: Send + Sync {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
    async fn shutdown(&mut self) -> io::Result<()>;
    fn peer_addr(&self) -> io::Result<SocketAddr>;
}

#[async_trait]
pub trait AsyncTcpListener: Send + Sync {
    async fn accept(&self) -> io::Result<(Box<dyn AsyncTcpStream>, SocketAddr)>;
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

#[async_trait]
pub trait AsyncTimer: Send {
    async fn wait(&mut self);
    fn deadline(&self) -> MonoTime;
}

pub trait JoinHandle: Send {
    fn abort(&self);
    fn is_finished(&self) -> bool;
    fn wait(
        self: Box<Self>,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskJoinError>> + Send + 'static>>;
}

pub trait Runtime: Send + Sync + 'static {
    type UdpSocket: AsyncUdpSocket;
    type TcpStream: AsyncTcpStream;
    type TcpListener: AsyncTcpListener;
    type Timer: AsyncTimer;
    type Handle: JoinHandle;

    fn now(&self) -> MonoTime;

    fn spawn(&self, fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) -> Self::Handle;
    fn spawn_local(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + 'static>>,
    ) -> Result<Self::Handle, SpawnError>;

    fn bind_udp(&self, addr: SocketAddr) -> io::Result<Self::UdpSocket>;

    fn connect_tcp(&self, addr: SocketAddr) -> io::Result<Self::TcpStream>;

    fn bind_tcp(&self, addr: SocketAddr) -> io::Result<Self::TcpListener>;

    fn wrap_udp_socket(&self, socket: StdUdpSocket) -> io::Result<Self::UdpSocket>;

    fn wrap_tcp_listener(&self, listener: StdTcpListener) -> io::Result<Self::TcpListener>;

    fn wrap_tcp_stream(&self, stream: StdTcpStream) -> io::Result<Self::TcpStream>;

    fn sleep_until(&self, deadline: MonoTime) -> Self::Timer;

    fn oneshot(&self) -> (OneShotSender, OneShotReceiver) {
        oneshot_channel()
    }
}

pub trait RuntimeApi: Send + Sync + 'static {
    fn now(&self) -> MonoTime;
    fn spawn(&self, fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>)
        -> Box<dyn JoinHandle>;
    fn spawn_local(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + 'static>>,
    ) -> Result<Box<dyn JoinHandle>, SpawnError>;
    fn bind_udp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncUdpSocket>>;
    fn connect_tcp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpStream>>;
    fn bind_tcp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpListener>>;
    fn wrap_udp_socket(&self, socket: StdUdpSocket) -> io::Result<Box<dyn AsyncUdpSocket>>;
    fn wrap_tcp_listener(&self, listener: StdTcpListener) -> io::Result<Box<dyn AsyncTcpListener>>;
    fn wrap_tcp_stream(&self, stream: StdTcpStream) -> io::Result<Box<dyn AsyncTcpStream>>;
    fn sleep_until(&self, deadline: MonoTime) -> Box<dyn AsyncTimer>;
    fn oneshot(&self) -> (OneShotSender, OneShotReceiver) {
        oneshot_channel()
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use futures::task::noop_waker;
    use std::task::{Context, Poll};

    use super::*;

    #[test]
    fn cancellation_cascades_to_children() {
        let root = CancellationToken::new();
        let child = root.child_token();
        let grandchild = child.child_token();
        assert!(!root.is_cancelled());
        assert!(!child.is_cancelled());
        assert!(!grandchild.is_cancelled());

        root.cancel();
        assert!(root.is_cancelled());
        assert!(child.is_cancelled());
        assert!(grandchild.is_cancelled());
    }

    #[test]
    fn oneshot_send_then_recv() {
        let (tx, mut rx) = oneshot_channel();
        tx.send().expect("send oneshot");
        block_on(async {
            rx.recv().await.expect("recv oneshot");
        });
    }

    #[test]
    fn dropped_cancellation_waiters_are_removed() {
        let token = CancellationToken::new();
        for _ in 0..256 {
            let mut fut = Box::pin(token.cancelled());
            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);
            assert!(matches!(fut.as_mut().poll(&mut cx), Poll::Pending));
        }
        assert!(token.inner.lock_waiters().is_empty());
    }

    #[test]
    fn dropped_child_tokens_are_pruned_on_next_child_creation() {
        let root = CancellationToken::new();
        {
            let _first = root.child_token();
            let _second = root.child_token();
        }
        {
            let children = root.inner.lock_children();
            assert_eq!(children.len(), 2);
        }
        let _live = root.child_token();
        let children = root.inner.lock_children();
        assert_eq!(children.len(), 1);
        assert!(children[0].upgrade().is_some());
    }
}
