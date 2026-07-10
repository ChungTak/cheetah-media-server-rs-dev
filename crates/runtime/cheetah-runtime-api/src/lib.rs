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

/// Errors that can occur when spawning a task on the runtime.
///
/// `LocalContextRequired` is returned when `spawn_local` is used on a runtime that
/// does not support non-`Send` tasks (e.g. the default Tokio multi-thread runtime).
///
/// 在运行时上生成任务时可能发生的错误。
///
/// `LocalContextRequired` 表示在不支持非 `Send` 任务的运行时上调用 `spawn_local`
///（如默认 Tokio 多线程运行时）。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SpawnError {
    #[error("local task context is required")]
    LocalContextRequired,
    #[error("{0}")]
    RuntimeUnavailable(String),
}

/// Error returned when a spawned task fails to complete successfully.
///
/// 生成任务未能成功完成时返回的错误。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct TaskJoinError {
    message: String,
}

impl TaskJoinError {
    /// Create a new join error with a descriptive message.
    ///
    /// 用描述性消息创建新的 join error。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Error returned when the oneshot sender is dropped before the receiver can receive.
///
/// 当 oneshot 发送者在接收者接收前被丢弃时返回的错误。
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("oneshot sender dropped before completion")]
pub struct OneShotRecvError;

/// Error returned when the oneshot receiver is dropped before the sender can send.
///
/// 当 oneshot 接收者在发送者发送前被丢弃时返回的错误。
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
    /// Signal completion. Fails if the receiver has already been dropped.
    ///
    /// 发送完成信号。若接收者已被丢弃则失败。
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
    /// 异步等待发送者发出完成信号。
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

/// Create a oneshot completion channel.
///
/// 创建 oneshot 完成通道。
pub fn oneshot_channel() -> (OneShotSender, OneShotReceiver) {
    let (tx, rx) = oneshot::channel();
    (OneShotSender { inner: tx }, OneShotReceiver { inner: rx })
}

/// Internal state shared by a `CancellationToken` and its waiters.
///
/// Tracks whether the token has been cancelled, the registered wakers, and weak
/// references to child tokens. Cancellation propagates recursively through children.
///
/// `CancellationToken` 及其等待者共享的内部状态。
///
/// 跟踪 token 是否已取消、已注册 waker 以及子 token 的弱引用。取消通过子 token 递归传播。
#[derive(Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    waiters: Mutex<Vec<Option<Waker>>>,
    children: Mutex<Vec<Weak<CancellationState>>>,
}

impl CancellationState {
    /// Create a new cancellation state.
    ///
    /// 创建新的取消状态。
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

/// Recursively cancel a token and all its descendants.
///
/// Wakes all registered waiters and propagates cancellation to child tokens. The
/// operation is idempotent; calling it on an already-cancelled root is a no-op.
///
/// 递归取消 token 及其所有后代。
///
/// 唤醒所有已注册等待者，并将取消传播到子 token。幂等操作；对已取消根调用无效果。
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

/// A token that can be used to request cancellation of a task or operation.
///
/// Child tokens can be created with `child_token`; cancelling a parent cancels all
/// of its descendants. Waiters use `cancelled()` to obtain a future that resolves when
/// cancellation is requested.
///
/// 用于请求取消任务或操作的 token。
///
/// 可用 `child_token` 创建子 token；取消父 token 会同时取消所有后代。
/// 等待者使用 `cancelled()` 获取一个在请求取消时完成的 future。
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
    /// Create a new root cancellation token.
    ///
    /// 创建新的根取消 token。
    pub fn new() -> Self {
        Self {
            inner: CancellationState::new(),
        }
    }

    /// Create a child token that inherits cancellation from this token.
    ///
    /// If the parent is already cancelled, the child is immediately cancelled. Dead
    /// child entries are pruned when new children are added.
    ///
    /// 创建继承当前 token 取消状态的子 token。
    ///
    /// 若父 token 已取消，子 token 立即取消。添加新子 token 时会清理已失效的弱引用。
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

    /// Request cancellation for this token and all its descendants.
    ///
    /// 请求取消该 token 及其所有后代。
    pub fn cancel(&self) {
        cancel_state(&self.inner);
    }

    /// Returns true if cancellation has been requested.
    ///
    /// 返回是否已请求取消。
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Return a future that resolves when the token is cancelled.
    ///
    /// 返回一个在 token 被取消时完成的 future。
    pub fn cancelled(&self) -> CancellationFuture {
        CancellationFuture {
            inner: self.inner.clone(),
            waiter_slot: None,
        }
    }
}

/// Future that resolves when the associated `CancellationToken` is cancelled.
///
/// Registers and deregisters a waker slot in the token's state. Dropped futures clean
/// up their slot to avoid leaking waker entries.
///
/// 当关联 `CancellationToken` 被取消时完成的 future。
///
/// 在 token 状态中注册和注销 waker 槽。被丢弃的 future 会清理其槽位，避免泄漏 waker。
pub struct CancellationFuture {
    inner: Arc<CancellationState>,
    waiter_slot: Option<usize>,
}

impl CancellationFuture {
    /// Remove the registered waker slot if present.
    ///
    /// 若已注册则移除 waker 槽。
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

/// Runtime-neutral async UDP socket.
///
/// Provides receive and send operations, local address lookup, and optional IPv4
/// multicast control. Implemented by concrete runtimes such as Tokio.
///
/// 运行时无关的异步 UDP socket。
///
/// 提供接收、发送、本地地址查询以及可选 IPv4 组播控制。由 Tokio 等具体运行时实现。
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

/// Runtime-neutral async TCP stream.
///
/// 运行时无关的异步 TCP 流。
#[async_trait]
pub trait AsyncTcpStream: Send + Sync {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
    async fn shutdown(&mut self) -> io::Result<()>;
    fn peer_addr(&self) -> io::Result<SocketAddr>;
}

/// Runtime-neutral async TCP listener.
///
/// 运行时无关的异步 TCP 监听器。
#[async_trait]
pub trait AsyncTcpListener: Send + Sync {
    async fn accept(&self) -> io::Result<(Box<dyn AsyncTcpStream>, SocketAddr)>;
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

/// Runtime-neutral async timer that resolves at a deadline.
///
/// 运行时无关的异步定时器，在截止时间到达时完成。
#[async_trait]
pub trait AsyncTimer: Send {
    async fn wait(&mut self);
    fn deadline(&self) -> MonoTime;
}

/// Handle returned by `spawn`/`spawn_local`, used to abort or await a task.
///
/// `spawn`/`spawn_local` 返回的句柄，用于中止或等待任务。
pub trait JoinHandle: Send {
    fn abort(&self);
    fn is_finished(&self) -> bool;
    fn wait(
        self: Box<Self>,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskJoinError>> + Send + 'static>>;
}

/// Runtime-neutral abstraction over async I/O, timers, and task spawning.
///
/// Each associated type is the concrete type provided by the runtime implementation.
/// This is the lower-level trait; user code usually interacts with `RuntimeApi`.
///
/// 异步 I/O、定时器和任务生成的运行时无关抽象。
///
/// 每个关联类型由运行时实现提供。这是底层 trait；用户代码通常通过 `RuntimeApi` 交互。
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

/// Object-safe, runtime-neutral API used by modules and the engine.
///
/// Provides the same capabilities as `Runtime` but through trait objects, so the
/// concrete runtime can be injected without monomorphizing the whole system.
///
/// 模块和引擎使用的对象安全、运行时无关 API。
///
/// 提供与 `Runtime` 相同的能力，但通过 trait 对象暴露，因此可以注入具体运行时
/// 而无需全系统单态化。
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
