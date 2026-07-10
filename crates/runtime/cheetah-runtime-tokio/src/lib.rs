use std::future::Future;
use std::io;
use std::net::{
    Ipv4Addr, SocketAddr, TcpListener as StdTcpListener, TcpStream as StdTcpStream,
    UdpSocket as StdUdpSocket,
};
use std::pin::Pin;
use std::time::Instant;

use async_trait::async_trait;
use cheetah_codec::MonoTime;
use cheetah_runtime_api::{
    oneshot_channel, AsyncTcpListener, AsyncTcpStream, AsyncTimer, AsyncUdpSocket, JoinHandle,
    OneShotReceiver, OneShotSender, Runtime, RuntimeApi, SpawnError, TaskJoinError, UdpRecvMeta,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::time::{sleep_until, Duration, Instant as TokioInstant, Sleep};

/// Apply low-latency TCP socket options for streaming protocols.
///
/// Sets `TCP_NODELAY` to bypass Nagle's algorithm on writes. On Linux we also
/// set `TCP_QUICKACK` so the kernel ACKs incoming data immediately instead of
/// applying the default 40 ms delayed-ACK. The combination is required to
/// avoid the classic Nagle (sender) × delayed-ACK (receiver) deadlock that
/// throttles RTSP-over-TCP-interleaved publish sessions to ~0.6× of wall-clock
/// once both audio and video tracks are present (the back-and-forth small
/// writes line up exactly with the delayed-ACK window).
fn apply_low_latency_tcp_options(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::fd::AsRawFd;
        let on: libc::c_int = 1;
        // TCP_QUICKACK is one-shot but requesting it disables further delayed-ACKs
        // for at least the next ACK, which is enough to keep small request/response
        // RTSP exchanges from stalling at the 40 ms ATO boundary on each turn.
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_QUICKACK,
            &on as *const _ as *const libc::c_void,
            std::mem::size_of_val(&on) as libc::socklen_t,
        );
    }
}

/// `TokioRuntime` data structure.
/// `TokioRuntime` 数据结构.
#[derive(Clone, Debug)]
pub struct TokioRuntime {
    /// `start` field of type `Instant`.
    /// `start` 字段，类型为 `Instant`.
    start: Instant,
    /// `start_tokio` field of type `TokioInstant`.
    /// `start_tokio` 字段，类型为 `TokioInstant`.
    start_tokio: TokioInstant,
}

impl TokioRuntime {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            start_tokio: TokioInstant::now(),
        }
    }

    fn deadline_to_instant(&self, deadline: MonoTime) -> TokioInstant {
        self.start_tokio + Duration::from_micros(deadline.as_micros())
    }
}

impl Default for TokioRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// `TokioJoinHandle` data structure.
/// `TokioJoinHandle` 数据结构.
#[derive(Debug)]
pub struct TokioJoinHandle {
    /// `handle` field.
    /// `handle` 字段.
    handle: tokio::task::JoinHandle<()>,
}

impl JoinHandle for TokioJoinHandle {
    fn abort(&self) {
        self.handle.abort();
    }

    fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    fn wait(
        self: Box<Self>,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskJoinError>> + Send + 'static>> {
        Box::pin(async move {
            let TokioJoinHandle { handle } = *self;
            handle
                .await
                .map_err(|err| TaskJoinError::new(err.to_string()))
        })
    }
}

/// `TokioUdpSocket` data structure.
/// `TokioUdpSocket` 数据结构.
#[derive(Debug)]
pub struct TokioUdpSocket {
    /// `socket` field of type `UdpSocket`.
    /// `socket` 字段，类型为 `UdpSocket`.
    socket: UdpSocket,
}

#[async_trait]
impl AsyncUdpSocket for TokioUdpSocket {
    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<UdpRecvMeta> {
        let (len, from) = self.socket.recv_from(buf).await?;
        Ok(UdpRecvMeta { from, len })
    }

    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
        self.socket.send_to(buf, target).await
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        self.socket.join_multicast_v4(multiaddr, interface)
    }

    fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        self.socket.leave_multicast_v4(multiaddr, interface)
    }

    fn set_multicast_ttl_v4(&self, ttl: u32) -> io::Result<()> {
        self.socket.set_multicast_ttl_v4(ttl)
    }

    fn set_multicast_loop_v4(&self, enabled: bool) -> io::Result<()> {
        self.socket.set_multicast_loop_v4(enabled)
    }
}

/// `TokioTcpStream` data structure.
/// `TokioTcpStream` 数据结构.
#[derive(Debug)]
pub struct TokioTcpStream {
    /// `stream` field of type `TcpStream`.
    /// `stream` 字段，类型为 `TcpStream`.
    stream: TcpStream,
}

#[async_trait]
impl AsyncTcpStream for TokioTcpStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.stream.read(buf).await?;
        // Re-arm TCP_QUICKACK after each successful read on Linux. The flag is
        // one-shot: once the next ACK is sent, the kernel reverts to delayed-ACK.
        // Streaming-style request/response loops need quick ACKs continuously to
        // avoid stalling the peer's Nagle-throttled writes at the 40 ms ATO.
        #[cfg(target_os = "linux")]
        if n > 0 {
            unsafe {
                use std::os::fd::AsRawFd;
                let on: libc::c_int = 1;
                libc::setsockopt(
                    self.stream.as_raw_fd(),
                    libc::IPPROTO_TCP,
                    libc::TCP_QUICKACK,
                    &on as *const _ as *const libc::c_void,
                    std::mem::size_of_val(&on) as libc::socklen_t,
                );
            }
        }
        Ok(n)
    }

    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.stream.write_all(buf).await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.stream.shutdown().await
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }
}

/// `TokioTcpListener` data structure.
/// `TokioTcpListener` 数据结构.
#[derive(Debug)]
pub struct TokioTcpListener {
    /// `listener` field of type `TcpListener`.
    /// `listener` 字段，类型为 `TcpListener`.
    listener: TcpListener,
}

#[async_trait]
impl AsyncTcpListener for TokioTcpListener {
    async fn accept(&self) -> io::Result<(Box<dyn AsyncTcpStream>, SocketAddr)> {
        let (stream, addr) = self.listener.accept().await?;
        apply_low_latency_tcp_options(&stream);
        Ok((Box::new(TokioTcpStream { stream }), addr))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

/// `TokioTimer` data structure.
/// `TokioTimer` 数据结构.
pub struct TokioTimer {
    /// `deadline` field of type `MonoTime`.
    /// `deadline` 字段，类型为 `MonoTime`.
    deadline: MonoTime,
    /// `sleep` field.
    /// `sleep` 字段.
    sleep: Pin<Box<Sleep>>,
}

#[async_trait]
impl AsyncTimer for TokioTimer {
    async fn wait(&mut self) {
        self.sleep.as_mut().await;
    }

    fn deadline(&self) -> MonoTime {
        self.deadline
    }
}

impl Runtime for TokioRuntime {
    type UdpSocket = TokioUdpSocket;
    type TcpStream = TokioTcpStream;
    type TcpListener = TokioTcpListener;
    type Timer = TokioTimer;
    type Handle = TokioJoinHandle;

    fn now(&self) -> MonoTime {
        MonoTime::from_micros(self.start.elapsed().as_micros() as u64)
    }

    fn spawn(&self, fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) -> Self::Handle {
        TokioJoinHandle {
            handle: tokio::spawn(fut),
        }
    }

    fn spawn_local(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + 'static>>,
    ) -> Result<Self::Handle, SpawnError> {
        let spawned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tokio::task::spawn_local(fut)
        }));
        match spawned {
            Ok(handle) => Ok(TokioJoinHandle { handle }),
            Err(_) => Err(SpawnError::LocalContextRequired),
        }
    }

    fn bind_udp(&self, addr: SocketAddr) -> io::Result<Self::UdpSocket> {
        let socket = StdUdpSocket::bind(addr)?;
        <Self as Runtime>::wrap_udp_socket(self, socket)
    }

    fn connect_tcp(&self, addr: SocketAddr) -> io::Result<Self::TcpStream> {
        let stream = StdTcpStream::connect(addr)?;
        <Self as Runtime>::wrap_tcp_stream(self, stream)
    }

    fn bind_tcp(&self, addr: SocketAddr) -> io::Result<Self::TcpListener> {
        let listener = StdTcpListener::bind(addr)?;
        <Self as Runtime>::wrap_tcp_listener(self, listener)
    }

    fn wrap_udp_socket(&self, socket: StdUdpSocket) -> io::Result<Self::UdpSocket> {
        socket.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(socket)?;
        Ok(TokioUdpSocket { socket })
    }

    fn wrap_tcp_listener(&self, listener: StdTcpListener) -> io::Result<Self::TcpListener> {
        listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(listener)?;
        Ok(TokioTcpListener { listener })
    }

    fn wrap_tcp_stream(&self, stream: StdTcpStream) -> io::Result<Self::TcpStream> {
        stream.set_nonblocking(true)?;
        let stream = TcpStream::from_std(stream)?;
        apply_low_latency_tcp_options(&stream);
        Ok(TokioTcpStream { stream })
    }

    fn sleep_until(&self, deadline: MonoTime) -> Self::Timer {
        let when = self.deadline_to_instant(deadline);
        TokioTimer {
            deadline,
            sleep: Box::pin(sleep_until(when)),
        }
    }

    fn oneshot(&self) -> (OneShotSender, OneShotReceiver) {
        oneshot_channel()
    }
}

impl RuntimeApi for TokioRuntime {
    fn now(&self) -> MonoTime {
        <Self as Runtime>::now(self)
    }

    fn spawn(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
    ) -> Box<dyn JoinHandle> {
        Box::new(<Self as Runtime>::spawn(self, fut))
    }

    fn spawn_local(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + 'static>>,
    ) -> Result<Box<dyn JoinHandle>, SpawnError> {
        Ok(Box::new(<Self as Runtime>::spawn_local(self, fut)?))
    }

    fn bind_udp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncUdpSocket>> {
        Ok(Box::new(<Self as Runtime>::bind_udp(self, addr)?))
    }

    fn connect_tcp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpStream>> {
        Ok(Box::new(<Self as Runtime>::connect_tcp(self, addr)?))
    }

    fn bind_tcp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpListener>> {
        Ok(Box::new(<Self as Runtime>::bind_tcp(self, addr)?))
    }

    fn wrap_udp_socket(&self, socket: StdUdpSocket) -> io::Result<Box<dyn AsyncUdpSocket>> {
        Ok(Box::new(<Self as Runtime>::wrap_udp_socket(self, socket)?))
    }

    fn wrap_tcp_listener(&self, listener: StdTcpListener) -> io::Result<Box<dyn AsyncTcpListener>> {
        Ok(Box::new(<Self as Runtime>::wrap_tcp_listener(
            self, listener,
        )?))
    }

    fn wrap_tcp_stream(&self, stream: StdTcpStream) -> io::Result<Box<dyn AsyncTcpStream>> {
        Ok(Box::new(<Self as Runtime>::wrap_tcp_stream(self, stream)?))
    }

    fn sleep_until(&self, deadline: MonoTime) -> Box<dyn AsyncTimer> {
        Box::new(<Self as Runtime>::sleep_until(self, deadline))
    }

    fn oneshot(&self) -> (OneShotSender, OneShotReceiver) {
        <Self as Runtime>::oneshot(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn spawn_local_requires_local_context() {
        let rt = TokioRuntime::new();
        let res = RuntimeApi::spawn_local(&rt, Box::pin(async {}));
        assert!(matches!(res, Err(SpawnError::LocalContextRequired)));
    }
}
