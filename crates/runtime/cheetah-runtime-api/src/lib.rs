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

/// Error returned when a task cannot be spawned on the runtime.
///
/// 当任务无法在运行时上生成时返回的错误。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SpawnError {
    /// The runtime requires a local (non-`Send`) task context.
    /// 运行时需要本地（非 `Send`）任务上下文。
    #[error("local task context is required")]
    LocalContextRequired,
    /// The runtime is unavailable with a descriptive message.
    /// 运行时不可用，附带描述信息。
    #[error("{0}")]
    RuntimeUnavailable(String),
}

/// Error returned when a spawned task fails to complete.
///
/// 当生成的任务未能完成时返回的错误。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct TaskJoinError {
    /// Human-readable failure message.
    /// 人类可读失败信息。
    message: String,
}

impl TaskJoinError {
    /// Create a new task join error.
    /// 创建新的任务加入错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Error returned when the receiver of a oneshot channel was dropped.
///
/// 当 oneshot 通道的接收端被丢弃时返回的错误。
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("oneshot sender dropped before completion")]
pub struct OneShotRecvError;

/// Error returned when the sender of a oneshot channel was dropped.
///
/// 当 oneshot 通道的发送端被丢弃时返回的错误。
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("oneshot receiver dropped before completion")]
pub struct OneShotSendError;

/// Sender half of a oneshot channel.
///
/// oneshot 通道的发送端。
#[derive(Debug)]
pub struct OneShotSender {
    inner: oneshot::Sender<()>,
}

impl OneShotSender {
    /// Signal completion to the receiver.
    /// 向接收端发送完成信号。
    pub fn send(self) -> Result<(), OneShotSendError> {
        self.inner.send(()).map_err(|_| OneShotSendError)
    }
}

/// Receiver half of a oneshot channel.
///
/// oneshot 通道的接收端。
#[derive(Debug)]
pub struct OneShotReceiver {
    inner: oneshot::Receiver<()>,
}

impl OneShotReceiver {
    /// Wait for the sender to signal completion.
    /// 等待发送端发出完成信号。
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

/// Create a new oneshot channel.
/// 创建新的 oneshot 通道。
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

/// Cooperative cancellation token used across the runtime-neutral API.
///
/// A token can be cancelled once, and all child tokens and `cancelled()` futures
/// are then notified. This is the primary mechanism for gracefully stopping modules.
///
/// 跨运行时无关 API 使用的协作取消令牌。
///
/// 令牌可被取消一次，随后所有子令牌和 `cancelled()` future 都会收到通知。
/// 这是优雅停止模块的主要机制。
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
    /// Create a new cancellation token.
    /// 创建新的取消令牌。
    pub fn new() -> Self {
        Self {
            inner: CancellationState::new(),
        }
    }

    /// Create a child token that is cancelled when this token is cancelled.
    /// 创建一个子令牌，当本令牌取消时子令牌也会被取消。
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

    /// Cancel this token and all its children.
    /// 取消本令牌及其所有子令牌。
    pub fn cancel(&self) {
        cancel_state(&self.inner);
    }

    /// Return `true` if the token has been cancelled.
    /// 返回令牌是否已被取消。
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Return a future that resolves when the token is cancelled.
    /// 返回一个当令牌被取消时完成的 future。
    pub fn cancelled(&self) -> CancellationFuture {
        CancellationFuture {
            inner: self.inner.clone(),
            waiter_slot: None,
        }
    }
}

/// Future that resolves when its associated cancellation token is cancelled.
///
/// 当关联取消令牌被取消时完成的 future。
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

/// Metadata returned by a UDP receive operation.
///
/// UDP 接收操作返回的元数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpRecvMeta {
    /// Source address of the packet.
    /// 数据包的源地址。
    pub from: SocketAddr,
    /// Number of bytes received.
    /// 接收到的字节数。
    pub len: usize,
}

/// Runtime-neutral async UDP socket.
///
/// 运行时无关的异步 UDP 套接字。
#[async_trait]
pub trait AsyncUdpSocket: Send + Sync {
    /// Receive a UDP packet into `buf` and return metadata.
    /// 将 UDP 数据包接收到 `buf` 中并返回元数据。
    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<UdpRecvMeta>;
    /// Send a UDP packet to `target`.
    /// 向 `target` 发送 UDP 数据包。
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize>;
    /// Return the local address of the socket.
    /// 返回套接字本地地址。
    fn local_addr(&self) -> io::Result<SocketAddr>;
    /// Join an IPv4 multicast group.
    /// 加入 IPv4 多播组。
    fn join_multicast_v4(&self, _multiaddr: Ipv4Addr, _interface: Ipv4Addr) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "join_multicast_v4 is not supported by this runtime socket",
        ))
    }
    /// Leave an IPv4 multicast group.
    /// 离开 IPv4 多播组。
    fn leave_multicast_v4(&self, _multiaddr: Ipv4Addr, _interface: Ipv4Addr) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "leave_multicast_v4 is not supported by this runtime socket",
        ))
    }
    /// Set the IPv4 multicast TTL.
    /// 设置 IPv4 多播 TTL。
    fn set_multicast_ttl_v4(&self, _ttl: u32) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "set_multicast_ttl_v4 is not supported by this runtime socket",
        ))
    }
    /// Enable or disable IPv4 multicast loopback.
    /// 启用或禁用 IPv4 多播环回。
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
    /// Read bytes into `buf`.
    /// 读取字节到 `buf`。
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    /// Write all bytes from `buf`.
    /// 写入 `buf` 中的所有字节。
    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
    /// Shut down the stream.
    /// 关闭流。
    async fn shutdown(&mut self) -> io::Result<()>;
    /// Return the peer address.
    /// 返回对端地址。
    fn peer_addr(&self) -> io::Result<SocketAddr>;
}

/// Runtime-neutral async TCP listener.
///
/// 运行时无关的异步 TCP 监听器。
#[async_trait]
pub trait AsyncTcpListener: Send + Sync {
    /// Accept a new connection.
    /// 接受新连接。
    async fn accept(&self) -> io::Result<(Box<dyn AsyncTcpStream>, SocketAddr)>;
    /// Return the local address.
    /// 返回本地地址。
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

/// Runtime-neutral async timer.
///
/// 运行时无关的异步定时器。
#[async_trait]
pub trait AsyncTimer: Send {
    /// Wait until the timer deadline.
    /// 等待直到定时器截止。
    async fn wait(&mut self);
    /// Return the absolute deadline of this timer.
    /// 返回此定时器的绝对截止时间。
    fn deadline(&self) -> MonoTime;
}

/// Handle for a spawned task.
///
/// 生成任务的句柄。
pub trait JoinHandle: Send {
    /// Abort the spawned task.
    /// 中止生成的任务。
    fn abort(&self);
    /// Return `true` if the task has finished.
    /// 返回任务是否已完成。
    fn is_finished(&self) -> bool;
    /// Wait for the task to finish and return its result.
    /// 等待任务完成并返回结果。
    fn wait(
        self: Box<Self>,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskJoinError>> + Send + 'static>>;
}

/// Runtime abstraction for spawning tasks, timers, and I/O.
///
/// `Runtime` is implemented once per concrete runtime (e.g. Tokio). It defines
/// the associated types for UDP sockets, TCP streams, listeners, timers, and
/// join handles. All modules use this trait through `RuntimeApi` to remain
/// runtime-neutral.
///
/// 用于生成任务、定时器和 I/O 的运行时抽象。
///
/// `Runtime` 每个具体运行时（如 Tokio）实现一次。它定义了 UDP 套接字、TCP 流、
/// 监听器、定时器和加入句柄的关联类型。所有模块通过 `RuntimeApi` 使用此 trait，
/// 以保持运行时无关。
pub trait Runtime: Send + Sync + 'static {
    type UdpSocket: AsyncUdpSocket;
    type TcpStream: AsyncTcpStream;
    type TcpListener: AsyncTcpListener;
    type Timer: AsyncTimer;
    type Handle: JoinHandle;

    /// Return the current monotonic time.
    /// 返回当前单调时间。
    fn now(&self) -> MonoTime;

    /// Spawn a `Send` future on the runtime.
    /// 在运行时上生成 `Send` future。
    fn spawn(&self, fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) -> Self::Handle;
    /// Spawn a non-`Send` future, returning an error if the runtime cannot support it.
    /// 生成非 `Send` future；若运行时不支持则返回错误。
    fn spawn_local(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + 'static>>,
    ) -> Result<Self::Handle, SpawnError>;

    /// Bind a UDP socket to the address.
    /// 将 UDP 套接字绑定到地址。
    fn bind_udp(&self, addr: SocketAddr) -> io::Result<Self::UdpSocket>;

    /// Connect a TCP stream to the address.
    /// 将 TCP 流连接到地址。
    fn connect_tcp(&self, addr: SocketAddr) -> io::Result<Self::TcpStream>;

    /// Bind a TCP listener to the address.
    /// 将 TCP 监听器绑定到地址。
    fn bind_tcp(&self, addr: SocketAddr) -> io::Result<Self::TcpListener>;

    /// Wrap an existing std UDP socket with the runtime.
    /// 用运行时包装现有 std UDP 套接字。
    fn wrap_udp_socket(&self, socket: StdUdpSocket) -> io::Result<Self::UdpSocket>;

    /// Wrap an existing std TCP listener with the runtime.
    /// 用运行时包装现有 std TCP 监听器。
    fn wrap_tcp_listener(&self, listener: StdTcpListener) -> io::Result<Self::TcpListener>;

    /// Wrap an existing std TCP stream with the runtime.
    /// 用运行时包装现有 std TCP 流。
    fn wrap_tcp_stream(&self, stream: StdTcpStream) -> io::Result<Self::TcpStream>;

    /// Create a timer that fires at `deadline`.
    /// 创建在 `deadline` 触发的定时器。
    fn sleep_until(&self, deadline: MonoTime) -> Self::Timer;

    /// Create a oneshot channel through the runtime.
    /// 通过运行时创建 oneshot 通道。
    fn oneshot(&self) -> (OneShotSender, OneShotReceiver) {
        oneshot_channel()
    }
}

/// Object-safe runtime API used by modules and the engine.
///
/// `RuntimeApi` is the dyn-compatible version of `Runtime`. It is passed to
/// modules through `EngineContext` so they can spawn tasks and perform I/O without
/// depending on Tokio or any other concrete runtime.
///
/// 模块和引擎使用的对象安全运行时 API。
///
/// `RuntimeApi` 是 `Runtime` 的 dyn-compatible 版本。它通过 `EngineContext` 传递给
/// 模块，使它们能够生成任务和执行 I/O，而无需依赖 Tokio 或其他具体运行时。
pub trait RuntimeApi: Send + Sync + 'static {
    /// Return the current monotonic time.
    /// 返回当前单调时间。
    fn now(&self) -> MonoTime;
    /// Spawn a `Send` future.
    /// 生成 `Send` future。
    fn spawn(&self, fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>)
        -> Box<dyn JoinHandle>;
    /// Spawn a non-`Send` future.
    /// 生成非 `Send` future。
    fn spawn_local(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + 'static>>,
    ) -> Result<Box<dyn JoinHandle>, SpawnError>;
    /// Bind a UDP socket.
    /// 绑定 UDP 套接字。
    fn bind_udp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncUdpSocket>>;
    /// Connect a TCP stream.
    /// 连接 TCP 流。
    fn connect_tcp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpStream>>;
    /// Bind a TCP listener.
    /// 绑定 TCP 监听器。
    fn bind_tcp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpListener>>;
    /// Wrap an existing std UDP socket.
    /// 包装现有 std UDP 套接字。
    fn wrap_udp_socket(&self, socket: StdUdpSocket) -> io::Result<Box<dyn AsyncUdpSocket>>;
    /// Wrap an existing std TCP listener.
    /// 包装现有 std TCP 监听器。
    fn wrap_tcp_listener(&self, listener: StdTcpListener) -> io::Result<Box<dyn AsyncTcpListener>>;
    /// Wrap an existing std TCP stream.
    /// 包装现有 std TCP 流。
    fn wrap_tcp_stream(&self, stream: StdTcpStream) -> io::Result<Box<dyn AsyncTcpStream>>;
    /// Create a timer that fires at `deadline`.
    /// 创建在 `deadline` 触发的定时器。
    fn sleep_until(&self, deadline: MonoTime) -> Box<dyn AsyncTimer>;
    /// Create a oneshot channel.
    /// 创建 oneshot 通道。
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
