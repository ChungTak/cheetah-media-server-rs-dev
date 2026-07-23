use async_trait::async_trait;
use cheetah_codec::MonoTime;
use cheetah_runtime_api::{
    oneshot_channel, AsyncTcpListener, AsyncTcpStream, AsyncTimer, AsyncUdpSocket,
    ConnectTcpFuture, ConnectTlsFuture, JoinHandle, OneShotReceiver, OneShotSender, Runtime,
    RuntimeApi, SpawnError, TaskJoinError, UdpRecvMeta,
};
use std::future::Future;
use std::io;
use std::net::{
    IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdTcpListener, TcpStream as StdTcpStream,
    UdpSocket as StdUdpSocket,
};
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpListener, TcpStream, UdpSocket};
use tokio::time::{sleep_until, Duration, Instant, Sleep};
use tokio_rustls::TlsConnector;

static TLS_CONFIG: LazyLock<Arc<rustls::ClientConfig>> = LazyLock::new(|| {
    // Installing the default crypto provider is idempotent; ignore failures from
    // multiple installs across test/runtime instantiations.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    )
});

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

/// Tokio-based `Runtime` implementation.
///
/// 基于 Tokio 的 `Runtime` 实现。
#[derive(Clone, Debug)]
pub struct TokioRuntime {
    start: Instant,
}

impl TokioRuntime {
    /// Create a runtime anchored at the current process start time.
    ///
    /// 创建以当前进程启动时间为锚点的运行时。
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Convert a `MonoTime` deadline to a Tokio `Instant`.
    ///
    /// 将 `MonoTime` 截止时间转换为 Tokio `Instant`。
    fn deadline_to_instant(&self, deadline: MonoTime) -> Instant {
        self.start + Duration::from_micros(deadline.as_micros())
    }

    /// Asynchronously connect to `addr` over plain TCP.
    ///
    /// 异步连接到 `addr` 的普通 TCP 连接。
    pub async fn connect_tcp_async(&self, addr: SocketAddr) -> io::Result<TokioTcpStream> {
        let stream = TcpStream::connect(addr).await?;
        apply_low_latency_tcp_options(&stream);
        Ok(TokioTcpStream { stream })
    }

    /// Asynchronously connect to `addr` and perform a TLS handshake using `server_name` as SNI.
    ///
    /// 异步连接到 `addr` 并使用 `server_name` 作为 SNI 完成 TLS 握手。
    pub async fn connect_tls(
        &self,
        addr: SocketAddr,
        server_name: &str,
    ) -> io::Result<TokioTlsTcpStream> {
        let tcp = TcpStream::connect(addr).await?;
        apply_low_latency_tcp_options(&tcp);
        let server_name = rustls::pki_types::ServerName::try_from(server_name.to_string())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        let connector = TlsConnector::from(TLS_CONFIG.clone());
        let stream = connector
            .connect(server_name, tcp)
            .await
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(TokioTlsTcpStream {
            stream: tokio_rustls::TlsStream::Client(stream),
        })
    }
}

impl Default for TokioRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Tokio task join handle wrapper.
///
/// Tokio 任务 join 句柄包装。
#[derive(Debug)]
pub struct TokioJoinHandle {
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

/// Tokio UDP socket wrapper.
///
/// Tokio UDP 套接字包装。
#[derive(Debug)]
pub struct TokioUdpSocket {
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

/// Tokio TCP stream wrapper with low-latency socket options.
///
/// 带低延迟套接字选项的 Tokio TCP 流包装。
#[derive(Debug)]
pub struct TokioTcpStream {
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

/// Tokio TLS-wrapped TCP stream.
///
/// 基于 Tokio Rustls 的 TLS TCP 流包装。
#[derive(Debug)]
pub struct TokioTlsTcpStream {
    stream: tokio_rustls::TlsStream<TcpStream>,
}

#[async_trait]
impl AsyncTcpStream for TokioTlsTcpStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf).await
    }

    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.stream.write_all(buf).await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.stream.shutdown().await
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.get_ref().0.peer_addr()
    }
}

#[derive(Debug)]
/// Tokio TCP listener wrapper.
///
/// Tokio TCP 监听器包装。
pub struct TokioTcpListener {
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

/// Tokio sleep-based timer.
///
/// 基于 Tokio sleep 的定时器。
pub struct TokioTimer {
    deadline: MonoTime,
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

/// `Runtime` implementation that bridges Tokio primitives to the runtime API.
///
/// 将 Tokio 原语桥接到运行时 API 的 `Runtime` 实现。
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

    fn spawn_blocking(
        &self,
        _name: &str,
        task: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<Self::Handle, SpawnError> {
        let handle = tokio::task::spawn_blocking(move || {
            task();
        });
        Ok(TokioJoinHandle { handle })
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

/// `RuntimeApi` implementation exposing Tokio primitives as trait objects.
///
/// 将 Tokio 原语作为 trait 对象暴露的 `RuntimeApi` 实现。
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

    fn spawn_blocking(
        &self,
        name: &str,
        task: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<Box<dyn JoinHandle>, SpawnError> {
        Ok(Box::new(<Self as Runtime>::spawn_blocking(
            self, name, task,
        )?))
    }

    fn bind_udp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncUdpSocket>> {
        Ok(Box::new(<Self as Runtime>::bind_udp(self, addr)?))
    }

    fn connect_tcp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpStream>> {
        Ok(Box::new(<Self as Runtime>::connect_tcp(self, addr)?))
    }

    fn connect_tcp_async<'a>(&'a self, addr: SocketAddr) -> ConnectTcpFuture<'a> {
        Box::pin(async move {
            let stream = TokioRuntime::connect_tcp_async(self, addr).await?;
            Ok(Box::new(stream) as Box<dyn AsyncTcpStream>)
        })
    }

    fn connect_tls<'a>(&'a self, addr: SocketAddr, server_name: &str) -> ConnectTlsFuture<'a> {
        let server_name = server_name.to_string();
        Box::pin(async move {
            let stream = TokioRuntime::connect_tls(self, addr, &server_name).await?;
            Ok(Box::new(stream) as Box<dyn AsyncTcpStream>)
        })
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

    fn resolve_host(
        &self,
        host: &str,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<IpAddr>>> + Send + '_>> {
        let host = host.to_string();
        Box::pin(async move {
            let query = format!("{host}:0");
            let addrs = lookup_host(query)
                .await?
                .map(|sa| sa.ip())
                .collect::<Vec<_>>();
            if addrs.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("host {host} resolved no addresses"),
                ));
            }
            Ok(addrs)
        })
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

    #[tokio::test(flavor = "current_thread")]
    async fn spawn_blocking_runs_and_joins() {
        let rt = TokioRuntime::new();
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag_clone = flag.clone();
        let handle = RuntimeApi::spawn_blocking(
            &rt,
            "test",
            Box::new(move || {
                flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            }),
        )
        .unwrap();
        handle.wait().await.unwrap();
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }
}
