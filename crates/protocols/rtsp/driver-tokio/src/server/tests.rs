use std::collections::HashSet;
use std::future::pending;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_rtsp_core::{RtspCommand, RtspEvent, RtspMethod};
use cheetah_runtime_api::{AsyncTcpStream, CancellationToken, RuntimeApi};
use cheetah_runtime_tokio::TokioRuntime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tokio::time::{timeout, Duration};

use super::command::{
    handle_driver_command, send_connection_command, ConnectionCommand, ConnectionHandle,
    ConnectionMap,
};
use super::connection::{run_connection, write_pending_bytes_for_test, ConnectionRuntime};
use super::*;

fn sample_command() -> RtspCommand {
    RtspCommand::SendResponse {
        cseq: Some(1),
        status_code: 200,
        reason: "OK".to_string(),
        headers: Vec::new(),
        body: Bytes::new(),
    }
}

struct PendingWriteStream {
    write_started: Arc<Notify>,
    shutdown_called: Arc<AtomicBool>,
    peer: SocketAddr,
}

impl PendingWriteStream {
    fn new(write_started: Arc<Notify>, shutdown_called: Arc<AtomicBool>) -> Self {
        Self {
            write_started,
            shutdown_called,
            peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 554),
        }
    }
}

fn bind_listen_addr() -> SocketAddr {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);
    listen
}

async fn wait_connection_open(handle: &mut RtspServerHandle) -> RtspConnectionId {
    timeout(Duration::from_secs(1), async {
        loop {
            match handle.recv_event().await {
                Some(DriverEvent::ConnectionOpened { connection_id, .. }) => {
                    break connection_id;
                }
                Some(_) => {}
                None => panic!("driver event channel closed"),
            }
        }
    })
    .await
    .expect("connection open timeout")
}

async fn wait_connection_closed_reason(
    handle: &mut RtspServerHandle,
    connection_id: RtspConnectionId,
) -> String {
    timeout(Duration::from_secs(2), async {
        loop {
            match handle.recv_event().await {
                Some(DriverEvent::ConnectionClosed {
                    connection_id: id,
                    reason,
                }) if id == connection_id => {
                    break reason;
                }
                Some(_) => {}
                None => panic!("driver event channel closed"),
            }
        }
    })
    .await
    .expect("connection close timeout")
}

async fn wait_core_event(
    handle: &mut RtspServerHandle,
    connection_id: RtspConnectionId,
) -> RtspEvent {
    timeout(Duration::from_secs(1), async {
        loop {
            match handle.recv_event().await {
                Some(DriverEvent::Core {
                    connection_id: id,
                    event,
                }) if id == connection_id => {
                    break event;
                }
                Some(_) => {}
                None => panic!("driver event channel closed"),
            }
        }
    })
    .await
    .expect("core event timeout")
}

async fn read_http_headers(stream: &mut tokio::net::TcpStream) -> String {
    let mut out = Vec::<u8>::new();
    loop {
        let mut one = [0_u8; 1];
        timeout(Duration::from_secs(1), stream.read_exact(&mut one))
            .await
            .expect("read header timeout")
            .expect("read header");
        out.push(one[0]);
        if out.len() >= 4 && out[out.len() - 4..] == *b"\r\n\r\n" {
            return String::from_utf8(out).expect("header utf8");
        }
    }
}

#[async_trait]
impl AsyncTcpStream for PendingWriteStream {
    async fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        pending::<io::Result<usize>>().await
    }

    async fn write_all(&mut self, _buf: &[u8]) -> io::Result<()> {
        self.write_started.notify_waiters();
        pending::<io::Result<()>>().await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.shutdown_called.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }
}

struct ReadAfterWriteStartsStream {
    read_payload: Option<Vec<u8>>,
    write_started_flag: Arc<AtomicBool>,
    write_started_notify: Arc<Notify>,
    shutdown_called: Arc<AtomicBool>,
    peer: SocketAddr,
}

impl ReadAfterWriteStartsStream {
    fn new(
        read_payload: Vec<u8>,
        write_started_flag: Arc<AtomicBool>,
        write_started_notify: Arc<Notify>,
        shutdown_called: Arc<AtomicBool>,
    ) -> Self {
        Self {
            read_payload: Some(read_payload),
            write_started_flag,
            write_started_notify,
            shutdown_called,
            peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 554),
        }
    }
}

#[async_trait]
impl AsyncTcpStream for ReadAfterWriteStartsStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.write_started_flag.load(Ordering::SeqCst) {
            self.write_started_notify.notified().await;
        }
        if let Some(payload) = self.read_payload.take() {
            let n = payload.len().min(buf.len());
            buf[..n].copy_from_slice(&payload[..n]);
            return Ok(n);
        }
        pending::<io::Result<usize>>().await
    }

    async fn write_all(&mut self, _buf: &[u8]) -> io::Result<()> {
        self.write_started_flag.store(true, Ordering::SeqCst);
        self.write_started_notify.notify_one();
        pending::<io::Result<()>>().await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.shutdown_called.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }
}

struct CountingWriteStream {
    write_count: Arc<AtomicUsize>,
    shutdown_called: Arc<AtomicBool>,
    peer: SocketAddr,
}

impl CountingWriteStream {
    fn new(write_count: Arc<AtomicUsize>, shutdown_called: Arc<AtomicBool>) -> Self {
        Self {
            write_count,
            shutdown_called,
            peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 554),
        }
    }
}

#[async_trait]
impl AsyncTcpStream for CountingWriteStream {
    async fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        pending::<io::Result<usize>>().await
    }

    async fn write_all(&mut self, _buf: &[u8]) -> io::Result<()> {
        self.write_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.shutdown_called.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }
}

struct PartialProgressWriteStream {
    written_prefix: Arc<Mutex<Vec<u8>>>,
    write_started: Arc<Notify>,
    shutdown_called: Arc<AtomicBool>,
    chunk_size: usize,
    peer: SocketAddr,
}

impl PartialProgressWriteStream {
    fn new(
        written_prefix: Arc<Mutex<Vec<u8>>>,
        write_started: Arc<Notify>,
        shutdown_called: Arc<AtomicBool>,
        chunk_size: usize,
    ) -> Self {
        Self {
            written_prefix,
            write_started,
            shutdown_called,
            chunk_size,
            peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 554),
        }
    }
}

#[async_trait]
impl AsyncTcpStream for PartialProgressWriteStream {
    async fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        pending::<io::Result<usize>>().await
    }

    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let n = self.chunk_size.min(buf.len());
        self.written_prefix
            .lock()
            .expect("written_prefix lock poisoned")
            .extend_from_slice(&buf[..n]);
        self.write_started.notify_one();
        pending::<io::Result<()>>().await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.shutdown_called.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }
}

struct GateControlledWriteStream {
    write_started: Arc<Notify>,
    allow_write_complete: Arc<Notify>,
    shutdown_called: Arc<AtomicBool>,
    peer: SocketAddr,
}

impl GateControlledWriteStream {
    fn new(
        write_started: Arc<Notify>,
        allow_write_complete: Arc<Notify>,
        shutdown_called: Arc<AtomicBool>,
    ) -> Self {
        Self {
            write_started,
            allow_write_complete,
            shutdown_called,
            peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 554),
        }
    }
}

#[async_trait]
impl AsyncTcpStream for GateControlledWriteStream {
    async fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        pending::<io::Result<usize>>().await
    }

    async fn write_all(&mut self, _buf: &[u8]) -> io::Result<()> {
        self.write_started.notify_one();
        self.allow_write_complete.notified().await;
        Ok(())
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.shutdown_called.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn accepts_connection_and_closes_by_command() {
    let listen = bind_listen_addr();
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let mut stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("tcp connect");

    let connection_id = wait_connection_open(&mut handle).await;

    stream
        .write_all(b"OPTIONS rtsp://127.0.0.1/live/test RTSP/1.0\r\nCSeq: 1\r\n\r\n")
        .await
        .expect("send options");

    handle
        .command_sender()
        .close_connection(connection_id)
        .await
        .expect("close connection command");

    let reason = wait_connection_closed_reason(&mut handle, connection_id).await;
    assert_eq!(reason, "closed by command");

    handle.shutdown();
    let _ = handle.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn sends_response_before_close_connection() {
    let listen = bind_listen_addr();
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let mut stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("tcp connect");

    let connection_id = wait_connection_open(&mut handle).await;

    handle
        .command_sender()
        .send_core(
            connection_id,
            RtspCommand::SendResponse {
                cseq: Some(1),
                status_code: 200,
                reason: "OK".to_string(),
                headers: Vec::new(),
                body: Bytes::new(),
            },
        )
        .await
        .expect("send response command");
    handle
        .command_sender()
        .close_connection(connection_id)
        .await
        .expect("close connection command");

    let mut buf = vec![0_u8; 2048];
    let n = timeout(Duration::from_secs(1), stream.read(&mut buf))
        .await
        .expect("read response timeout")
        .expect("read response failed");
    assert!(n > 0, "driver closed before writing response");
    let response = std::str::from_utf8(&buf[..n]).expect("response utf8");
    assert!(response.contains("RTSP/1.0 200 OK"));

    handle.shutdown();
    let _ = handle.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn parses_rtsp_and_interleaved_from_same_segment() {
    let listen = bind_listen_addr();
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let mut stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("tcp connect");
    let connection_id = wait_connection_open(&mut handle).await;

    let mut mixed_segment =
        b"OPTIONS rtsp://127.0.0.1/live/test RTSP/1.0\r\nCSeq: 21\r\n\r\n".to_vec();
    mixed_segment.extend_from_slice(&[b'$', 2, 0, 4, b'A', b'B', b'C', b'D']);
    stream
        .write_all(&mixed_segment)
        .await
        .expect("send mixed segment");

    let event1 = wait_core_event(&mut handle, connection_id).await;
    let event2 = wait_core_event(&mut handle, connection_id).await;
    match event1 {
        RtspEvent::Request(request) => {
            assert_eq!(request.method, RtspMethod::Options);
            assert_eq!(request.cseq, Some(21));
        }
        _ => panic!("first event should be request"),
    }
    match event2 {
        RtspEvent::InterleavedFrame { channel, payload } => {
            assert_eq!(channel, 2);
            assert_eq!(payload, Bytes::from_static(b"ABCD"));
        }
        _ => panic!("second event should be interleaved frame"),
    }

    handle
        .command_sender()
        .close_connection(connection_id)
        .await
        .expect("close connection command");
    let _ = wait_connection_closed_reason(&mut handle, connection_id).await;
    handle.shutdown();
    let _ = handle.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn buffers_segmented_rtsp_message_until_complete() {
    let listen = bind_listen_addr();
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let mut stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("tcp connect");
    let connection_id = wait_connection_open(&mut handle).await;

    stream
        .write_all(b"OPTIONS rtsp://127.0.0.1/live/test RTSP/1.0\r\nCSeq: 3")
        .await
        .expect("send partial rtsp request");
    let no_event = timeout(Duration::from_millis(120), handle.recv_event()).await;
    assert!(
        no_event.is_err(),
        "partial rtsp message should stay buffered without emitting event"
    );

    stream
        .write_all(b"0\r\n\r\n")
        .await
        .expect("send rtsp request tail");
    let event = wait_core_event(&mut handle, connection_id).await;
    match event {
        RtspEvent::Request(request) => {
            assert_eq!(request.method, RtspMethod::Options);
            assert_eq!(request.cseq, Some(30));
        }
        _ => panic!("expected request event"),
    }

    handle
        .command_sender()
        .close_connection(connection_id)
        .await
        .expect("close connection command");
    let _ = wait_connection_closed_reason(&mut handle, connection_id).await;
    handle.shutdown();
    let _ = handle.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn buffers_partial_interleaved_frame_until_payload_complete() {
    let listen = bind_listen_addr();
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let mut stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("tcp connect");
    let connection_id = wait_connection_open(&mut handle).await;

    stream
        .write_all(&[b'$', 7, 0, 4, b'A', b'B'])
        .await
        .expect("send partial interleaved frame");
    let no_event = timeout(Duration::from_millis(120), handle.recv_event()).await;
    assert!(
        no_event.is_err(),
        "partial interleaved payload should stay buffered without emitting event"
    );

    stream
        .write_all(b"CD")
        .await
        .expect("complete interleaved payload");
    let event = wait_core_event(&mut handle, connection_id).await;
    match event {
        RtspEvent::InterleavedFrame { channel, payload } => {
            assert_eq!(channel, 7);
            assert_eq!(payload, Bytes::from_static(b"ABCD"));
        }
        _ => panic!("expected interleaved frame event"),
    }

    handle
        .command_sender()
        .close_connection(connection_id)
        .await
        .expect("close connection command");
    let _ = wait_connection_closed_reason(&mut handle, connection_id).await;
    handle.shutdown();
    let _ = handle.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn buffer_limit_hit_closes_connection_with_traceable_reason() {
    let listen = bind_listen_addr();
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let mut stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("tcp connect");
    let connection_id = wait_connection_open(&mut handle).await;

    let oversized = vec![b'x'; 1024 * 1024 + 2048];
    let _ = stream.write_all(&oversized).await;

    let reason = wait_connection_closed_reason(&mut handle, connection_id).await;
    assert!(
        reason.contains("buffer size limit exceeded"),
        "unexpected close reason: {reason}"
    );

    handle.shutdown();
    let _ = handle.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn peer_close_emits_peer_closed_reason() {
    let listen = bind_listen_addr();
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("tcp connect");
    let connection_id = wait_connection_open(&mut handle).await;
    drop(stream);

    let reason = wait_connection_closed_reason(&mut handle, connection_id).await;
    assert_eq!(reason, "peer closed");

    handle.shutdown();
    let _ = handle.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn close_connection_cancels_even_when_command_queue_full() {
    let server_cancel = CancellationToken::new();
    let connection_cancel = server_cancel.child_token();
    let (tx, _rx) = mpsc::channel(1);
    tx.try_send(ConnectionCommand::Core(sample_command()))
        .expect("fill connection queue");

    let connection_id = 7;
    let conn_map: ConnectionMap =
        Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
    conn_map.lock().insert(
        connection_id,
        ConnectionHandle {
            tx,
            cancel: connection_cancel.clone(),
        },
    );

    let should_stop = handle_driver_command(
        RtspDriverCommand::CloseConnection { connection_id },
        &conn_map,
        &server_cancel,
    )
    .await;

    assert!(!should_stop);
    assert!(connection_cancel.is_cancelled());
    assert!(conn_map.lock().get(&connection_id).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn full_core_command_queue_forces_connection_close() {
    let server_cancel = CancellationToken::new();
    let connection_cancel = server_cancel.child_token();
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send(ConnectionCommand::Core(sample_command()))
        .expect("fill connection queue");

    let connection_id = 9;
    let conn_map: ConnectionMap =
        Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
    conn_map.lock().insert(
        connection_id,
        ConnectionHandle {
            tx,
            cancel: connection_cancel.clone(),
        },
    );

    send_connection_command(
        connection_id,
        ConnectionCommand::Core(sample_command()),
        &conn_map,
    );
    assert!(connection_cancel.is_cancelled());
    assert!(conn_map.lock().get(&connection_id).is_none());

    match rx.try_recv() {
        Ok(ConnectionCommand::Core(_)) => {}
        Ok(ConnectionCommand::Close) => panic!("unexpected close command"),
        Err(err) => panic!("expected initial queued command: {err}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_direct_connection_cleans_up_and_stops_when_event_channel_closed() {
    let connection_id = 77;
    let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 554);
    let (_event_tx, event_rx) = mpsc::channel::<DriverEvent>(1);
    let event_tx = _event_tx;
    drop(event_rx);
    let conn_map: ConnectionMap =
        Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let join_cancel = CancellationToken::new();
    let shutdown_called = Arc::new(AtomicBool::new(false));
    let stream = Box::new(CountingWriteStream::new(
        Arc::new(AtomicUsize::new(0)),
        shutdown_called.clone(),
    ));
    let should_stop = super::listener::spawn_direct_connection(
        connection_id,
        peer,
        stream,
        Bytes::new(),
        &event_tx,
        &conn_map,
        &join_cancel,
        &DriverConfig::default(),
        &runtime_api,
    )
    .await;

    assert!(should_stop);
    assert!(shutdown_called.load(Ordering::SeqCst));
    assert!(conn_map.lock().get(&connection_id).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn queue_backpressure_does_not_block_dispatch_loop() {
    let server_cancel = CancellationToken::new();
    let connection1_id = 11;
    let connection1_cancel = server_cancel.child_token();

    let (tx1, mut rx1) = mpsc::channel(1);
    tx1.try_send(ConnectionCommand::Core(sample_command()))
        .expect("fill connection1 queue");

    let conn_map: ConnectionMap =
        Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
    conn_map.lock().insert(
        connection1_id,
        ConnectionHandle {
            tx: tx1,
            cancel: connection1_cancel.clone(),
        },
    );

    let stop = timeout(
        Duration::from_millis(50),
        handle_driver_command(
            RtspDriverCommand::Core {
                connection_id: connection1_id,
                command: sample_command(),
            },
            &conn_map,
            &server_cancel,
        ),
    )
    .await
    .expect("driver dispatch should not block on a full connection queue");
    assert!(!stop);
    assert!(connection1_cancel.is_cancelled());
    assert!(conn_map.lock().get(&connection1_id).is_none());
    match rx1.try_recv() {
        Ok(ConnectionCommand::Core(_)) => {}
        Ok(ConnectionCommand::Close) => panic!("unexpected close command"),
        Err(err) => panic!("expected original queued command: {err}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_interrupts_blocked_write_and_emits_connection_closed() {
    let connection_id = 41;
    let runtime_cancel = CancellationToken::new();
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let conn_map: ConnectionMap =
        Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
    conn_map.lock().insert(
        connection_id,
        ConnectionHandle {
            tx: cmd_tx.clone(),
            cancel: runtime_cancel.clone(),
        },
    );

    let write_started = Arc::new(Notify::new());
    let shutdown_called = Arc::new(AtomicBool::new(false));
    let stream = Box::new(PendingWriteStream::new(
        write_started.clone(),
        shutdown_called.clone(),
    ));
    let runtime = ConnectionRuntime {
        event_tx,
        conn_map: conn_map.clone(),
        cancel: runtime_cancel.clone(),
        config: DriverConfig::default(),
    };

    let join = tokio::spawn(async move {
        run_connection(connection_id, stream, Bytes::new(), cmd_rx, runtime).await;
    });

    cmd_tx
        .send(ConnectionCommand::Core(sample_command()))
        .await
        .expect("send core response command");
    timeout(Duration::from_secs(1), write_started.notified())
        .await
        .expect("write did not start");

    runtime_cancel.cancel();

    let closed_reason = timeout(Duration::from_secs(1), async {
        loop {
            match event_rx.recv().await {
                Some(DriverEvent::ConnectionClosed {
                    connection_id: id,
                    reason,
                }) if id == connection_id => {
                    break reason;
                }
                Some(_) => {}
                None => panic!("event channel closed before connection close event"),
            }
        }
    })
    .await
    .expect("timed out waiting connection closed event");

    assert_eq!(closed_reason, "cancelled");
    timeout(Duration::from_secs(1), join)
        .await
        .expect("connection task did not stop")
        .expect("connection task join failed");
    assert!(shutdown_called.load(Ordering::SeqCst));
    assert!(conn_map.lock().get(&connection_id).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn close_command_waits_for_pending_write_and_emits_closed_by_command() {
    let connection_id = 42;
    let runtime_cancel = CancellationToken::new();
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let conn_map: ConnectionMap =
        Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
    conn_map.lock().insert(
        connection_id,
        ConnectionHandle {
            tx: cmd_tx.clone(),
            cancel: runtime_cancel.clone(),
        },
    );

    let write_started = Arc::new(Notify::new());
    let allow_write_complete = Arc::new(Notify::new());
    let shutdown_called = Arc::new(AtomicBool::new(false));
    let stream = Box::new(GateControlledWriteStream::new(
        write_started.clone(),
        allow_write_complete.clone(),
        shutdown_called.clone(),
    ));
    let runtime = ConnectionRuntime {
        event_tx,
        conn_map: conn_map.clone(),
        cancel: runtime_cancel.clone(),
        config: DriverConfig::default(),
    };

    let join = tokio::spawn(async move {
        run_connection(connection_id, stream, Bytes::new(), cmd_rx, runtime).await;
    });

    cmd_tx
        .send(ConnectionCommand::Core(sample_command()))
        .await
        .expect("send core response command");
    timeout(Duration::from_secs(1), write_started.notified())
        .await
        .expect("write did not start");
    cmd_tx
        .send(ConnectionCommand::Close)
        .await
        .expect("send close command");

    let close_before_write_done = timeout(Duration::from_millis(120), async {
        loop {
            match event_rx.recv().await {
                Some(DriverEvent::ConnectionClosed {
                    connection_id: id, ..
                }) if id == connection_id => break,
                Some(_) => {}
                None => panic!("event channel closed before connection close event"),
            }
        }
    })
    .await;
    assert!(
        close_before_write_done.is_err(),
        "close command should not cancel non-cancel-safe write_all mid-flight"
    );

    allow_write_complete.notify_one();

    let closed_reason = timeout(Duration::from_secs(1), async {
        loop {
            match event_rx.recv().await {
                Some(DriverEvent::ConnectionClosed {
                    connection_id: id,
                    reason,
                }) if id == connection_id => break reason,
                Some(_) => {}
                None => panic!("event channel closed before connection close event"),
            }
        }
    })
    .await
    .expect("timed out waiting connection closed event");

    assert_eq!(closed_reason, "closed by command");
    timeout(Duration::from_secs(1), join)
        .await
        .expect("connection task did not stop")
        .expect("connection task join failed");
    assert!(shutdown_called.load(Ordering::SeqCst));
    assert!(conn_map.lock().get(&connection_id).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn close_requested_with_pending_write_does_not_timeout() {
    let write_started = Arc::new(Notify::new());
    let shutdown_called = Arc::new(AtomicBool::new(false));
    let cancel = CancellationToken::new();
    let stream = Box::new(PendingWriteStream::new(
        write_started.clone(),
        shutdown_called.clone(),
    ));

    let mut join = tokio::spawn(write_pending_bytes_for_test(stream, cancel.clone(), true));
    timeout(Duration::from_secs(1), write_started.notified())
        .await
        .expect("write did not start");

    let premature_finish = timeout(Duration::from_millis(180), &mut join).await;
    assert!(
        premature_finish.is_err(),
        "pending write finished under close request; write path should wait for write_all or cancellation"
    );

    cancel.cancel();
    let cancel_result = timeout(Duration::from_secs(1), join)
        .await
        .expect("write task did not stop")
        .expect("write task join failed");
    assert_eq!(cancel_result, Err("cancelled".to_string()));
    assert!(!shutdown_called.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_read_peer_data_while_write_all_is_inflight() {
    let connection_id = 51;
    let runtime_cancel = CancellationToken::new();
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let conn_map: ConnectionMap =
        Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
    conn_map.lock().insert(
        connection_id,
        ConnectionHandle {
            tx: cmd_tx.clone(),
            cancel: runtime_cancel.clone(),
        },
    );

    let write_started = Arc::new(Notify::new());
    let write_started_flag = Arc::new(AtomicBool::new(false));
    let shutdown_called = Arc::new(AtomicBool::new(false));
    let request = b"OPTIONS rtsp://127.0.0.1/live/test RTSP/1.0\r\nCSeq: 71\r\n\r\n".to_vec();
    let stream = Box::new(ReadAfterWriteStartsStream::new(
        request,
        write_started_flag.clone(),
        write_started.clone(),
        shutdown_called.clone(),
    ));
    let runtime = ConnectionRuntime {
        event_tx,
        conn_map: conn_map.clone(),
        cancel: runtime_cancel.clone(),
        config: DriverConfig::default(),
    };

    let join = tokio::spawn(async move {
        run_connection(connection_id, stream, Bytes::new(), cmd_rx, runtime).await;
    });

    cmd_tx
        .send(ConnectionCommand::Core(sample_command()))
        .await
        .expect("send response command");
    timeout(Duration::from_secs(1), write_started.notified())
        .await
        .expect("write path was not entered");

    let no_event = timeout(Duration::from_millis(120), async {
        loop {
            match event_rx.recv().await {
                Some(DriverEvent::Core {
                    connection_id: id, ..
                }) if id == connection_id => break,
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    })
    .await;
    assert!(
        no_event.is_err(),
        "read path should not preempt a pending non-cancel-safe write_all"
    );

    runtime_cancel.cancel();
    let closed_reason = timeout(Duration::from_secs(1), async {
        loop {
            match event_rx.recv().await {
                Some(DriverEvent::ConnectionClosed {
                    connection_id: id,
                    reason,
                }) if id == connection_id => break reason,
                Some(_) => {}
                None => panic!("event channel closed before connection closed"),
            }
        }
    })
    .await
    .expect("timed out waiting connection close");
    assert_eq!(closed_reason, "cancelled");
    timeout(Duration::from_secs(1), join)
        .await
        .expect("connection task did not stop")
        .expect("connection task join failed");
    assert!(shutdown_called.load(Ordering::SeqCst));
    assert!(conn_map.lock().get(&connection_id).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn command_heavy_load_still_flushes_socket_writes() {
    let connection_id = 52;
    let runtime_cancel = CancellationToken::new();
    let (event_tx, mut event_rx) = mpsc::channel(16);
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    let conn_map: ConnectionMap =
        Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
    conn_map.lock().insert(
        connection_id,
        ConnectionHandle {
            tx: cmd_tx.clone(),
            cancel: runtime_cancel.clone(),
        },
    );

    let write_count = Arc::new(AtomicUsize::new(0));
    let shutdown_called = Arc::new(AtomicBool::new(false));
    let stream = Box::new(CountingWriteStream::new(
        write_count.clone(),
        shutdown_called.clone(),
    ));
    let runtime = ConnectionRuntime {
        event_tx,
        conn_map: conn_map.clone(),
        cancel: runtime_cancel.clone(),
        config: DriverConfig {
            write_queue_capacity: 64,
            command_queue_capacity: 64,
            event_queue_capacity: 1024,
            read_buffer_size: 64 * 1024,
            ..DriverConfig::default()
        },
    };

    let join = tokio::spawn(async move {
        run_connection(connection_id, stream, Bytes::new(), cmd_rx, runtime).await;
    });

    let producer_cmd_tx = cmd_tx.clone();
    let producer = tokio::spawn(async move {
        loop {
            if producer_cmd_tx
                .send(ConnectionCommand::Core(sample_command()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        write_count.load(Ordering::Relaxed) > 0,
        "writes should progress even when command queue stays busy"
    );

    let maybe_closed = timeout(Duration::from_millis(50), async {
        loop {
            match event_rx.recv().await {
                Some(DriverEvent::ConnectionClosed {
                    connection_id: id,
                    reason,
                }) if id == connection_id => break Some(reason),
                Some(_) => {}
                None => break None,
            }
        }
    })
    .await;
    assert!(
        maybe_closed.is_err(),
        "connection unexpectedly closed under command load: {:?}",
        maybe_closed.ok().flatten()
    );

    producer.abort();
    runtime_cancel.cancel();
    let _ = timeout(Duration::from_secs(1), async {
        loop {
            match event_rx.recv().await {
                Some(DriverEvent::ConnectionClosed {
                    connection_id: id, ..
                }) if id == connection_id => break,
                Some(_) => {}
                None => break,
            }
        }
    })
    .await;

    timeout(Duration::from_secs(1), join)
        .await
        .expect("connection task did not stop")
        .expect("connection task join failed");
    assert!(shutdown_called.load(Ordering::SeqCst));
    assert!(conn_map.lock().get(&connection_id).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_retry_non_cancel_safe_write_all_when_write_is_pending() {
    let connection_id = 53;
    let runtime_cancel = CancellationToken::new();
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let conn_map: ConnectionMap =
        Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
    conn_map.lock().insert(
        connection_id,
        ConnectionHandle {
            tx: cmd_tx.clone(),
            cancel: runtime_cancel.clone(),
        },
    );

    let chunk_size = 5;
    let written_prefix = Arc::new(Mutex::new(Vec::new()));
    let write_started = Arc::new(Notify::new());
    let shutdown_called = Arc::new(AtomicBool::new(false));
    let stream = Box::new(PartialProgressWriteStream::new(
        written_prefix.clone(),
        write_started.clone(),
        shutdown_called.clone(),
        chunk_size,
    ));
    let runtime = ConnectionRuntime {
        event_tx,
        conn_map: conn_map.clone(),
        cancel: runtime_cancel.clone(),
        config: DriverConfig::default(),
    };

    let join = tokio::spawn(async move {
        run_connection(connection_id, stream, Bytes::new(), cmd_rx, runtime).await;
    });

    cmd_tx
        .send(ConnectionCommand::Core(sample_command()))
        .await
        .expect("send core response command");
    timeout(Duration::from_secs(1), write_started.notified())
        .await
        .expect("write did not start");

    tokio::time::sleep(Duration::from_millis(30)).await;
    let written_len = written_prefix
        .lock()
        .expect("written_prefix lock poisoned")
        .len();
    assert_eq!(
        written_len, chunk_size,
        "pending write retried after partial progress: expected one prefix write of {chunk_size} bytes, got {written_len} bytes"
    );

    runtime_cancel.cancel();
    let closed_reason = timeout(Duration::from_secs(1), async {
        loop {
            match event_rx.recv().await {
                Some(DriverEvent::ConnectionClosed {
                    connection_id: id,
                    reason,
                }) if id == connection_id => break reason,
                Some(_) => {}
                None => panic!("event channel closed before connection closed"),
            }
        }
    })
    .await
    .expect("timed out waiting connection close");
    assert_eq!(closed_reason, "cancelled");

    timeout(Duration::from_secs(1), join)
        .await
        .expect("connection task did not stop")
        .expect("connection task join failed");
    assert!(shutdown_called.load(Ordering::SeqCst));
    assert!(conn_map.lock().get(&connection_id).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_command_closes_all_active_connections() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let _stream1 = tokio::net::TcpStream::connect(listen)
        .await
        .expect("tcp connect stream1");
    let _stream2 = tokio::net::TcpStream::connect(listen)
        .await
        .expect("tcp connect stream2");

    let opened_ids = timeout(Duration::from_secs(1), async {
        let mut ids = HashSet::new();
        while ids.len() < 2 {
            match handle.recv_event().await {
                Some(DriverEvent::ConnectionOpened { connection_id, .. }) => {
                    ids.insert(connection_id);
                }
                Some(_) => {}
                None => panic!("driver event channel closed"),
            }
        }
        ids
    })
    .await
    .expect("connection open timeout");

    handle
        .send_command(RtspDriverCommand::Shutdown)
        .await
        .expect("send shutdown command");

    let closed_ids = timeout(Duration::from_secs(1), async {
        let mut ids = HashSet::new();
        while ids.len() < 2 {
            match handle.recv_event().await {
                Some(DriverEvent::ConnectionClosed { connection_id, .. }) => {
                    ids.insert(connection_id);
                }
                Some(_) => {}
                None => panic!("driver event channel closed"),
            }
        }
        ids
    })
    .await
    .expect("connection close timeout");

    assert_eq!(closed_ids, opened_ids);
    let _ = handle.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_tunnel_get_can_arrive_after_initial_probe_timeout() {
    let listen = bind_listen_addr();
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let mut get_stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("connect get");
    tokio::time::sleep(Duration::from_millis(40)).await;
    get_stream
        .write_all(b"GET /live/tunnel-delay HTTP/1.0\r\nx-sessioncookie: delayed-cookie\r\n\r\n")
        .await
        .expect("write delayed get");
    let get_headers = read_http_headers(&mut get_stream).await;
    assert!(
        get_headers.starts_with("HTTP/1.0 200 OK"),
        "unexpected GET response: {get_headers}"
    );

    let mut post_stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("connect post");
    post_stream
        .write_all(
            b"POST /live/tunnel-delay HTTP/1.0\r\nx-sessioncookie: delayed-cookie\r\ncontent-type: application/x-rtsp-tunnelled\r\ncontent-length: 0\r\n\r\n",
        )
        .await
        .expect("write post");
    let post_headers = read_http_headers(&mut post_stream).await;
    assert!(
        post_headers.starts_with("HTTP/1.0 200 OK"),
        "unexpected POST response: {post_headers}"
    );

    let connection_id = wait_connection_open(&mut handle).await;
    handle
        .command_sender()
        .close_connection(connection_id)
        .await
        .expect("close tunnel connection");
    let _ = wait_connection_closed_reason(&mut handle, connection_id).await;
    handle.shutdown();
    let _ = handle.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn incomplete_http_tunnel_candidate_does_not_stall_accept_loop() {
    let listen = bind_listen_addr();
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let mut stalled = tokio::net::TcpStream::connect(listen)
        .await
        .expect("connect stalled candidate");
    stalled
        .write_all(b"GET /live/stall HTTP/1.0\r\nx-sessioncookie: stall")
        .await
        .expect("write incomplete tunnel candidate");

    let mut rtsp_stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("connect rtsp peer");
    rtsp_stream
        .write_all(b"OPTIONS rtsp://127.0.0.1/live/test RTSP/1.0\r\nCSeq: 7\r\n\r\n")
        .await
        .expect("write rtsp request");

    let rtsp_connection_id = timeout(Duration::from_secs(1), async {
        loop {
            match handle.recv_event().await {
                Some(DriverEvent::Core {
                    connection_id,
                    event: RtspEvent::Request(request),
                }) if request.method == RtspMethod::Options && request.cseq == Some(7) => {
                    break connection_id;
                }
                Some(_) => {}
                None => panic!("driver event channel closed"),
            }
        }
    })
    .await
    .expect("rtsp request should not be blocked by stalled tunnel candidate");

    handle
        .command_sender()
        .close_connection(rtsp_connection_id)
        .await
        .expect("close rtsp connection");
    let _ = wait_connection_closed_reason(&mut handle, rtsp_connection_id).await;
    drop(stalled);

    handle.shutdown();
    let _ = handle.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn tunnel_probe_timeout_keeps_bytes_for_direct_rtsp_handshake() {
    let listen = bind_listen_addr();
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let mut stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("connect");
    tokio::time::sleep(Duration::from_millis(40)).await;

    stream
        .write_all(b"OPTIONS rtsp://127.0.0.1/live/test RTSP/1.0\r\nCSeq: 19")
        .await
        .expect("write partial request");
    tokio::time::sleep(Duration::from_millis(300)).await;
    stream
        .write_all(b"\r\n\r\n")
        .await
        .expect("write request tail");

    let connection_id = wait_connection_open(&mut handle).await;
    let event = wait_core_event(&mut handle, connection_id).await;
    match event {
        RtspEvent::Request(request) => {
            assert_eq!(request.method, RtspMethod::Options);
            assert_eq!(request.cseq, Some(19));
        }
        _ => panic!("expected RTSP request event after probe timeout"),
    }

    handle
        .command_sender()
        .close_connection(connection_id)
        .await
        .expect("close command");
    let _ = wait_connection_closed_reason(&mut handle, connection_id).await;
    handle.shutdown();
    let _ = handle.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn silent_connection_probe_does_not_block_accept_loop() {
    let listen = bind_listen_addr();
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let mut handle = start_server(
        runtime_api,
        listen,
        DriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start server");

    let _silent = tokio::net::TcpStream::connect(listen)
        .await
        .expect("connect silent peer");
    tokio::time::sleep(Duration::from_millis(30)).await;

    let mut rtsp_stream = tokio::net::TcpStream::connect(listen)
        .await
        .expect("connect rtsp peer");
    rtsp_stream
        .write_all(b"OPTIONS rtsp://127.0.0.1/live/test RTSP/1.0\r\nCSeq: 71\r\n\r\n")
        .await
        .expect("write rtsp request");

    let rtsp_connection_id = timeout(Duration::from_millis(220), async {
        loop {
            match handle.recv_event().await {
                Some(DriverEvent::Core {
                    connection_id,
                    event: RtspEvent::Request(request),
                }) if request.method == RtspMethod::Options && request.cseq == Some(71) => {
                    break connection_id;
                }
                Some(_) => {}
                None => panic!("driver event channel closed"),
            }
        }
    })
    .await
    .expect("rtsp request should not be blocked by silent probe");

    handle
        .command_sender()
        .close_connection(rtsp_connection_id)
        .await
        .expect("close rtsp connection");
    let _ = wait_connection_closed_reason(&mut handle, rtsp_connection_id).await;

    handle.shutdown();
    let _ = handle.wait().await;
}
