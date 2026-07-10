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

/// Error returned when a runtime cannot spawn a task.
///
/// 运行时在无法派生任务时返回的错误。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SpawnError {
    #[error("local task context is required")]
    LocalContextRequired,
    #[error("{0}")]
    RuntimeUnavailable(String),
}

/// Error returned when joining a spawned task fails.
///
/// 加入已派生任务失败时返回的错误。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct TaskJoinError {
    message: String,
}

impl TaskJoinError {
    /// Build a join error with a descriptive message.
    ///
    /// 使用描述性消息构建 join 错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Error returned when the receiver side of a oneshot channel is dropped early.
///
/// 当 oneshot 通道的接收端提前被丢弃时返回的错误。
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("oneshot sender dropped before completion")]
pub struct OneShotRecvError;

/// Error returned when the sender side of a oneshot channel is dropped early.
///
/// 当 oneshot 通道的发送端提前被丢弃时返回的错误。
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("oneshot receiver dropped before completion")]
pub struct OneShotSendError;

/// Sender half of a oneshot completion channel.
///
/// oneshot 完成通道的发送端。
#[derive(Debug)]
pub struct OneShotSender {
    inner: oneshot::Sender<()>,
}

impl OneShotSender {
    /// Signal completion to the receiver, returning an error if the receiver was dropped.
    ///
    /// 向接收端发送完成信号；如果接收端已被丢弃则返回错误。
    pub fn send(self) -> Result<(), OneShotSendError> {
        self.inner.send(()).map_err(|_| OneShotSendError)
    }
}

/// Receiver half of a oneshot completion channel.
///
/// oneshot 完成通道的接收端。
#[derive(Debug)]
pub struct OneShotReceiver {
    inner: oneshot::Receiver<()>,
}

impl OneShotReceiver {
    /// Wait asynchronously for the sender to signal completion.
    ///
    /// 异步等待发送端发出完成信号。
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

/// Create a new oneshot completion channel.
///
/// 创建一个新的 oneshot 完成通道。
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

/// Runtime-neutral cancellation token that can be cloned and linked to children.
///
/// Cancelling a token marks itself and all linked child tokens as cancelled, waking
/// any `CancellationFuture` waiters.
///
/// 可克隆并可链接到子 token 的运行时无关取消 token。
///
/// 取消一个 token 会标记自身及所有链接的子 token 为已取消，并唤醒任何
/// `CancellationFuture` 等待者。
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
    /// Create a new token that is not yet cancelled.
    ///
    /// 创建一个尚未取消的新 token。
    pub fn new() -> Self {
        Self {
            inner: CancellationState::new(),
        }
    }

    /// Create a child token that propagates cancellation from this token.
    ///
    /// If the parent is already cancelled, the child is cancelled immediately. The parent
    /// keeps a weak reference to each child and prunes dead ones on subsequent calls.
    ///
    /// 创建一个继承此 token 取消状态的子 token。
    ///
    /// 如果父 token 已取消，子 token 会立即被取消。父 token 保留对每个子 token 的弱引用，
    /// 并在后续调用中清理已失效的引用。
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

    /// Cancel this token and cascade cancellation to all linked children.
    ///
    /// 取消此 token 并将取消状态级联到所有链接的子 token。
    pub fn cancel(&self) {
        cancel_state(&self.inner);
    }

    /// Return whether this token has been cancelled.
    ///
    /// 返回此 token 是否已被取消。
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Return a future that resolves when the token is cancelled.
    ///
    /// 返回一个在此 token 被取消时完成的 future。
    pub fn cancelled(&self) -> CancellationFuture {
        CancellationFuture {
            inner: self.inner.clone(),
            waiter_slot: None,
        }
    }
}

/// Future that resolves once the associated cancellation token is cancelled.
///
/// Deregistering the waker on drop prevents leaked waiter slots.
///
/// 关联的取消 token 被取消后完成的 future。
///
/// 在 drop 时注销 waker 可防止等待槽泄漏。
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

/// Metadata for a UDP receive operation.
///
/// UDP 接收操作的元数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpRecvMeta {
    pub from: SocketAddr,
    pub len: usize,
}

/// Runtime-neutral async UDP socket interface.
///
/// 运行时无关的异步 UDP 套接字接口。
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

/// Runtime-neutral async TCP stream interface.
///
/// 运行时无关的异步 TCP 流接口。
#[async_trait]
pub trait AsyncTcpStream: Send + Sync {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
    async fn shutdown(&mut self) -> io::Result<()>;
    fn peer_addr(&self) -> io::Result<SocketAddr>;
}

/// Runtime-neutral async TCP listener interface.
///
/// 运行时无关的异步 TCP 监听器接口。
#[async_trait]
pub trait AsyncTcpListener: Send + Sync {
    async fn accept(&self) -> io::Result<(Box<dyn AsyncTcpStream>, SocketAddr)>;
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

/// Runtime-neutral timer that resolves at a specific monotonic time.
///
/// 运行时无关的计时器，在指定单调时间到达时完成。
#[async_trait]
pub trait AsyncTimer: Send {
    async fn wait(&mut self);
    fn deadline(&self) -> MonoTime;
}

/// Handle returned by `spawn` or `spawn_local` that can be aborted or awaited.
///
/// `spawn` 或 `spawn_local` 返回的句柄，可用于中止或等待任务完成。
pub trait JoinHandle: Send {
    /// Abort the spawned task.
    ///
    /// 中止已派生的任务。
    fn abort(&self);
    /// Return whether the spawned task has already finished.
    ///
    /// 返回已派生任务是否已完成。
    fn is_finished(&self) -> bool;

    /// Return a future that resolves when the spawned task finishes.
    ///
    /// 返回一个已派生任务完成时解析的 future。
    fn wait(
        self: Box<Self>,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskJoinError>> + Send + 'static>>;
}

/// Concrete runtime trait with associated socket/timer/handle types.
///
/// Implementors provide a full async runtime for the engine and drivers.
///
/// 具有关联套接字/计时器/句柄类型的具体运行时 trait。
///
/// 实现者提供一个完整的引擎和驱动器异步运行时。
pub trait Runtime: Send + Sync + 'static {
    type UdpSocket: AsyncUdpSocket;
    type TcpStream: AsyncTcpStream;
    type TcpListener: AsyncTcpListener;
    type Timer: AsyncTimer;
    type Handle: JoinHandle;

    /// Return the current monotonic time.
    ///
    /// 返回当前单调时间。
    fn now(&self) -> MonoTime;

    /// Spawn a `Send` future on the runtime thread pool.
    ///
    /// 在线程池上派生一个 `Send` future。
    fn spawn(&self, fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) -> Self::Handle;

    /// Spawn a non-`Send` future on the local task context.
    ///
    /// Returns `SpawnError::LocalContextRequired` if called outside a local context.
    ///
    /// 在本地任务上下文上派生一个非 `Send` future。
    ///
    /// 如果在本地上下文外调用，返回 `SpawnError::LocalContextRequired`。
    fn spawn_local(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + 'static>>,
    ) -> Result<Self::Handle, SpawnError>;

    /// Bind a UDP socket to the given address.
    ///
    /// 将 UDP 套接字绑定到给定地址。
    fn bind_udp(&self, addr: SocketAddr) -> io::Result<Self::UdpSocket>;

    /// Connect a TCP stream to the given address.
    ///
    /// 连接 TCP 流到给定地址。
    fn connect_tcp(&self, addr: SocketAddr) -> io::Result<Self::TcpStream>;

    /// Bind a TCP listener to the given address.
    ///
    /// 将 TCP 监听器绑定到给定地址。
    fn bind_tcp(&self, addr: SocketAddr) -> io::Result<Self::TcpListener>;

    /// Adopt an existing UDP socket into the runtime.
    ///
    /// 将现有 UDP 套接字接入运行时。
    fn wrap_udp_socket(&self, socket: StdUdpSocket) -> io::Result<Self::UdpSocket>;

    /// Adopt an existing TCP listener into the runtime.
    ///
    /// 将现有 TCP 监听器接入运行时。
    fn wrap_tcp_listener(&self, listener: StdTcpListener) -> io::Result<Self::TcpListener>;

    /// Adopt an existing TCP stream into the runtime.
    ///
    /// 将现有 TCP 流接入运行时。
    fn wrap_tcp_stream(&self, stream: StdTcpStream) -> io::Result<Self::TcpStream>;

    /// Create a timer that resolves at the given monotonic deadline.
    ///
    /// 创建一个在指定单调时间到达时完成的计时器。
    fn sleep_until(&self, deadline: MonoTime) -> Self::Timer;

    /// Create a new oneshot completion channel.
    ///
    /// 创建一个新的 oneshot 完成通道。
    fn oneshot(&self) -> (OneShotSender, OneShotReceiver) {
        oneshot_channel()
    }
}

/// Object-safe runtime abstraction used by modules and the engine.
///
/// This is the type-erased version of `Runtime`, allowing different runtimes
/// to be injected without exposing concrete socket/timer types.
///
/// 模块和引擎使用的对象安全运行时抽象。
///
/// 它是 `Runtime` 的类型擦除版本，允许注入不同的运行时而无需暴露具体套接字/计时器类型。
pub trait RuntimeApi: Send + Sync + 'static {
    /// Return the current monotonic time.
    ///
    /// 返回当前单调时间。
    fn now(&self) -> MonoTime;

    /// Spawn a `Send` future and return a type-erased join handle.
    ///
    /// 派生一个 `Send` future 并返回类型擦除的 join 句柄。
    fn spawn(&self, fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>)
        -> Box<dyn JoinHandle>;

    /// Spawn a non-`Send` future on the local task context.
    ///
    /// 在本地任务上下文上派生一个非 `Send` future。
    fn spawn_local(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + 'static>>,
    ) -> Result<Box<dyn JoinHandle>, SpawnError>;

    /// Bind a UDP socket and return it as a trait object.
    ///
    /// 绑定 UDP 套接字并作为 trait 对象返回。
    fn bind_udp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncUdpSocket>>;

    /// Connect a TCP stream and return it as a trait object.
    ///
    /// 连接 TCP 流并作为 trait 对象返回。
    fn connect_tcp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpStream>>;

    /// Bind a TCP listener and return it as a trait object.
    ///
    /// 绑定 TCP 监听器并作为 trait 对象返回。
    fn bind_tcp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpListener>>;

    /// Adopt an existing UDP socket and return it as a trait object.
    ///
    /// 接入现有 UDP 套接字并作为 trait 对象返回。
    fn wrap_udp_socket(&self, socket: StdUdpSocket) -> io::Result<Box<dyn AsyncUdpSocket>>;

    /// Adopt an existing TCP listener and return it as a trait object.
    ///
    /// 接入现有 TCP 监听器并作为 trait 对象返回。
    fn wrap_tcp_listener(&self, listener: StdTcpListener) -> io::Result<Box<dyn AsyncTcpListener>>;

    /// Adopt an existing TCP stream and return it as a trait object.
    ///
    /// 接入现有 TCP 流并作为 trait 对象返回。
    fn wrap_tcp_stream(&self, stream: StdTcpStream) -> io::Result<Box<dyn AsyncTcpStream>>;

    /// Create a timer that resolves at the given monotonic deadline.
    ///
    /// 创建在指定单调时间到达时完成的计时器。
    fn sleep_until(&self, deadline: MonoTime) -> Box<dyn AsyncTimer>;

    /// Create a new oneshot completion channel.
    ///
    /// 创建一个新的 oneshot 完成通道。
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
