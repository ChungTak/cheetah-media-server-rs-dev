use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use bytes::Bytes;
use cheetah_runtime_api::{AsyncUdpSocket, CancellationToken, JoinHandle, RuntimeApi};

use super::RtspClientEvent;

/// Inclusive UDP port range for client RTP/RTCP allocation.
///
/// 客户端 RTP/RTCP 分配的包含性 UDP 端口范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtspClientPortRange {
    pub start: u16,
    pub end: u16,
}

/// A pair of bound UDP sockets representing the RTP and RTCP sides of one track.
///
/// RTP sockets are bound to even ports and the RTCP socket to the next odd port.
///
/// 表示一个轨道的 RTP 与 RTCP 两侧的一组已绑定 UDP 套接字。
///
/// RTP 套接字绑定在偶数端口，RTCP 套接字绑定在下一个奇数端口。
#[derive(Clone)]
pub struct RtspClientUdpEndpoint {
    pub rtp_socket: Arc<dyn AsyncUdpSocket>,
    pub rtcp_socket: Arc<dyn AsyncUdpSocket>,
    pub local_rtp: SocketAddr,
    pub local_rtcp: SocketAddr,
}

/// Remote UDP addresses for RTP and RTCP traffic of one track.
///
/// 单个轨道的 RTP 与 RTCP 远端 UDP 地址。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtspClientUdpRemote {
    pub rtp: SocketAddr,
    pub rtcp: SocketAddr,
}

/// Allocate a paired RTP/RTCP UDP endpoint.
///
/// If `port_range` is `Some`, scans the configured range for an even-odd pair.
/// Otherwise, binds an ephemeral even-odd pair by probing the OS repeatedly.
///
/// 分配一对 RTP/RTCP UDP 端点。
///
/// 若 `port_range` 为 `Some`，则在配置范围内扫描偶-奇端口对；否则通过反复探测
/// 操作系统绑定临时的偶-奇端口对。
pub fn allocate_udp_endpoint(
    runtime_api: &Arc<dyn RuntimeApi>,
    bind_ip: IpAddr,
    port_range: Option<RtspClientPortRange>,
) -> io::Result<RtspClientUdpEndpoint> {
    let (rtp_socket, rtcp_socket) = match port_range {
        Some(range) => bind_udp_pair_in_range(runtime_api, bind_ip, range)?,
        None => bind_udp_ephemeral_pair(runtime_api, bind_ip)?,
    };

    let local_rtp = rtp_socket.local_addr()?;
    let local_rtcp = rtcp_socket.local_addr()?;
    Ok(RtspClientUdpEndpoint {
        rtp_socket,
        rtcp_socket,
        local_rtp,
        local_rtcp,
    })
}

/// Send one-byte probes to the remote RTP and RTCP addresses to open NAT pinholes.
///
/// This must happen before the remote peer starts sending media, otherwise the first
/// inbound UDP packets may be dropped by stateful firewalls or NAT gateways.
///
/// 向远端 RTP 和 RTCP 地址发送单字节探测包以打通 NAT 洞。
///
/// 必须在远端开始发送媒体之前执行，否则首个入站 UDP 包可能被状态防火墙或
/// NAT 网关丢弃。
pub async fn configure_udp_remote_and_punch(
    endpoint: &RtspClientUdpEndpoint,
    remote_rtp: SocketAddr,
    remote_rtcp: SocketAddr,
) -> io::Result<()> {
    // Send one-byte payload to open NAT pinholes before media starts.
    let _ = endpoint.rtp_socket.send_to(&[0], remote_rtp).await?;
    let _ = endpoint.rtcp_socket.send_to(&[0], remote_rtcp).await?;
    Ok(())
}

/// Spawn two background tasks that receive RTP and RTCP packets for one track.
///
/// Each task loops on `recv_from`, optionally filters by the expected remote source,
/// and forwards the payload as `RtspClientEvent::UdpRtp` or `RtspClientEvent::UdpRtcp`.
/// Both tasks share the same cancellation token parent but each has an independent
/// child token so a single `cancel` stops both.
///
/// 为单个轨道生成两个后台任务，分别接收 RTP 与 RTCP 包。
///
/// 每个任务循环调用 `recv_from`，可选按预期远端源过滤，并将负载转发为
/// `RtspClientEvent::UdpRtp` 或 `RtspClientEvent::UdpRtcp`。两者共享同一个父取消令牌，
/// 但各自拥有独立的子令牌，因此一次 `cancel` 即可停止两个任务。
pub fn spawn_udp_receive_tasks(
    runtime_api: Arc<dyn RuntimeApi>,
    endpoint: RtspClientUdpEndpoint,
    track_id: u32,
    expected_remote: Option<RtspClientUdpRemote>,
    event_tx: tokio::sync::mpsc::Sender<RtspClientEvent>,
    cancel: CancellationToken,
) -> Vec<Box<dyn JoinHandle>> {
    let rtp_socket = endpoint.rtp_socket.clone();
    let rtcp_socket = endpoint.rtcp_socket.clone();
    let rtp_cancel = cancel.child_token();
    let rtcp_cancel = cancel.child_token();
    let rtp_event_tx = event_tx.clone();

    let rtp_join = runtime_api.spawn(Box::pin(async move {
        let mut buf = vec![0_u8; 64 * 1024];
        loop {
            let recv = tokio::select! {
                _ = rtp_cancel.cancelled() => break,
                recv = rtp_socket.recv_from(&mut buf) => recv,
            };
            let Ok(meta) = recv else {
                break;
            };
            if meta.len == 0 || meta.len > buf.len() {
                continue;
            }
            if expected_remote.is_some_and(|remote| meta.from != remote.rtp) {
                continue;
            }
            if rtp_event_tx
                .send(RtspClientEvent::UdpRtp {
                    track_id,
                    from: meta.from,
                    payload: Bytes::copy_from_slice(&buf[..meta.len]),
                })
                .await
                .is_err()
            {
                break;
            }
        }
    }));

    let rtcp_join = runtime_api.spawn(Box::pin(async move {
        let mut buf = vec![0_u8; 16 * 1024];
        loop {
            let recv = tokio::select! {
                _ = rtcp_cancel.cancelled() => break,
                recv = rtcp_socket.recv_from(&mut buf) => recv,
            };
            let Ok(meta) = recv else {
                break;
            };
            if meta.len == 0 || meta.len > buf.len() {
                continue;
            }
            if expected_remote.is_some_and(|remote| meta.from != remote.rtcp) {
                continue;
            }
            if event_tx
                .send(RtspClientEvent::UdpRtcp {
                    track_id,
                    from: meta.from,
                    payload: Bytes::copy_from_slice(&buf[..meta.len]),
                })
                .await
                .is_err()
            {
                break;
            }
        }
    }));

    vec![rtp_join, rtcp_join]
}

/// Bind an ephemeral even-odd UDP port pair.
///
/// Picks a random even port, tries to bind the next odd port, and retries up to 512
/// times. This mirrors the RTP convention where RTP uses an even port and RTCP uses
/// the following odd port.
///
/// 绑定临时的偶-奇 UDP 端口对。
///
/// 随机选择一个偶数端口，尝试绑定下一个奇数端口，最多重试 512 次。这遵循 RTP
/// 约定：RTP 使用偶数端口，RTCP 使用紧随其后的奇数端口。
fn bind_udp_ephemeral_pair(
    runtime_api: &Arc<dyn RuntimeApi>,
    bind_ip: IpAddr,
) -> io::Result<(Arc<dyn AsyncUdpSocket>, Arc<dyn AsyncUdpSocket>)> {
    for _ in 0..512 {
        let Ok(rtp) = runtime_api.bind_udp(SocketAddr::new(bind_ip, 0)) else {
            continue;
        };
        let Ok(local_rtp) = rtp.local_addr() else {
            drop(rtp);
            continue;
        };
        let rtp_port = local_rtp.port();
        if !rtp_port.is_multiple_of(2) || rtp_port == u16::MAX {
            drop(rtp);
            continue;
        }
        let rtcp_addr = SocketAddr::new(bind_ip, rtp_port.saturating_add(1));
        let Ok(rtcp) = runtime_api.bind_udp(rtcp_addr) else {
            drop(rtp);
            continue;
        };
        return Ok((Arc::from(rtp), Arc::from(rtcp)));
    }

    Err(io::Error::new(
        io::ErrorKind::AddrNotAvailable,
        "no ephemeral udp port pair available",
    ))
}

/// Bind an even-odd UDP port pair constrained to a user-supplied range.
///
/// Rounds the start up to the next even port and walks the range in steps of two.
/// If no pair is free, returns `AddrNotAvailable`.
///
/// 在用户指定范围内绑定偶-奇 UDP 端口对。
///
/// 将起始端口向上取整到下一个偶数，并以步长 2 遍历范围。若没有可用对，返回
/// `AddrNotAvailable`。
fn bind_udp_pair_in_range(
    runtime_api: &Arc<dyn RuntimeApi>,
    bind_ip: IpAddr,
    range: RtspClientPortRange,
) -> io::Result<(Arc<dyn AsyncUdpSocket>, Arc<dyn AsyncUdpSocket>)> {
    if range.start == 0 || range.end == 0 || range.start > range.end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid udp port range",
        ));
    }
    let start = if range.start.is_multiple_of(2) {
        range.start
    } else {
        range.start.saturating_add(1)
    };

    let mut port = start;
    while port < range.end {
        let rtp_addr = SocketAddr::new(bind_ip, port);
        let rtcp_addr = SocketAddr::new(bind_ip, port.saturating_add(1));
        let Ok(rtp) = runtime_api.bind_udp(rtp_addr) else {
            port = port.saturating_add(2);
            continue;
        };
        let Ok(rtcp) = runtime_api.bind_udp(rtcp_addr) else {
            drop(rtp);
            port = port.saturating_add(2);
            continue;
        };
        return Ok((Arc::from(rtp), Arc::from(rtcp)));
    }

    Err(io::Error::new(
        io::ErrorKind::AddrNotAvailable,
        "no udp port pair available in configured range",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_runtime_tokio::TokioRuntime;
    use std::net::{IpAddr, Ipv4Addr, UdpSocket};
    use tokio::sync::mpsc;
    use tokio::time::{timeout, Duration};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn allocate_udp_endpoint_uses_even_odd_pair_in_range() {
        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let endpoint = allocate_udp_endpoint(
            &runtime_api,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            Some(RtspClientPortRange {
                start: 43_000,
                end: 43_010,
            }),
        )
        .expect("allocate endpoint");
        assert_eq!(endpoint.local_rtp.port() % 2, 0);
        assert_eq!(endpoint.local_rtcp.port(), endpoint.local_rtp.port() + 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn allocate_udp_endpoint_uses_even_odd_pair_without_range() {
        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());

        for _ in 0..8 {
            let endpoint =
                allocate_udp_endpoint(&runtime_api, IpAddr::V4(Ipv4Addr::LOCALHOST), None)
                    .expect("allocate endpoint");
            assert_eq!(endpoint.local_rtp.port() % 2, 0);
            assert_eq!(endpoint.local_rtcp.port(), endpoint.local_rtp.port() + 1);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configure_udp_remote_and_punch_sends_probe_packets() {
        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let endpoint = allocate_udp_endpoint(&runtime_api, IpAddr::V4(Ipv4Addr::LOCALHOST), None)
            .expect("allocate endpoint");

        let rtp_listener = UdpSocket::bind("127.0.0.1:0").expect("bind rtp listener");
        let rtcp_listener = UdpSocket::bind("127.0.0.1:0").expect("bind rtcp listener");
        rtp_listener
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .expect("set read timeout");
        rtcp_listener
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .expect("set read timeout");

        configure_udp_remote_and_punch(
            &endpoint,
            rtp_listener.local_addr().expect("rtp addr"),
            rtcp_listener.local_addr().expect("rtcp addr"),
        )
        .await
        .expect("punch");

        let mut buf = [0u8; 8];
        let rtp_n = rtp_listener.recv(&mut buf).expect("recv rtp probe");
        let rtcp_n = rtcp_listener.recv(&mut buf).expect("recv rtcp probe");
        assert_eq!(rtp_n, 1);
        assert_eq!(rtcp_n, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_udp_receive_tasks_emits_rtp_and_rtcp_events() {
        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let endpoint = allocate_udp_endpoint(&runtime_api, IpAddr::V4(Ipv4Addr::LOCALHOST), None)
            .expect("allocate endpoint");
        let track_id = 7;
        let cancel = CancellationToken::new();
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let mut joins = spawn_udp_receive_tasks(
            runtime_api.clone(),
            endpoint.clone(),
            track_id,
            None,
            event_tx,
            cancel.clone(),
        );

        let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
        sender
            .send_to(b"rtp-data", endpoint.local_rtp)
            .expect("send rtp");
        sender
            .send_to(b"rtcp-data", endpoint.local_rtcp)
            .expect("send rtcp");

        let mut saw_rtp = false;
        let mut saw_rtcp = false;
        let res = timeout(Duration::from_secs(2), async {
            while !saw_rtp || !saw_rtcp {
                match event_rx.recv().await {
                    Some(RtspClientEvent::UdpRtp {
                        track_id: id,
                        payload,
                        ..
                    }) => {
                        assert_eq!(id, track_id);
                        assert_eq!(payload.as_ref(), b"rtp-data");
                        saw_rtp = true;
                    }
                    Some(RtspClientEvent::UdpRtcp {
                        track_id: id,
                        payload,
                        ..
                    }) => {
                        assert_eq!(id, track_id);
                        assert_eq!(payload.as_ref(), b"rtcp-data");
                        saw_rtcp = true;
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await;
        assert!(res.is_ok(), "did not receive udp events in time");

        cancel.cancel();
        for join in joins.drain(..) {
            let _ = join.wait().await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_udp_receive_tasks_filters_unexpected_remote_sources() {
        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let endpoint = allocate_udp_endpoint(&runtime_api, IpAddr::V4(Ipv4Addr::LOCALHOST), None)
            .expect("allocate endpoint");
        let track_id = 7;
        let cancel = CancellationToken::new();
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let expected_rtp = UdpSocket::bind("127.0.0.1:0").expect("bind expected rtp sender");
        let expected_rtcp = UdpSocket::bind("127.0.0.1:0").expect("bind expected rtcp sender");
        let unexpected = UdpSocket::bind("127.0.0.1:0").expect("bind unexpected sender");
        let mut joins = spawn_udp_receive_tasks(
            runtime_api.clone(),
            endpoint.clone(),
            track_id,
            Some(RtspClientUdpRemote {
                rtp: expected_rtp.local_addr().expect("expected rtp addr"),
                rtcp: expected_rtcp.local_addr().expect("expected rtcp addr"),
            }),
            event_tx,
            cancel.clone(),
        );

        unexpected
            .send_to(b"bad-rtp", endpoint.local_rtp)
            .expect("send unexpected rtp");
        unexpected
            .send_to(b"bad-rtcp", endpoint.local_rtcp)
            .expect("send unexpected rtcp");
        assert!(
            timeout(Duration::from_millis(200), event_rx.recv())
                .await
                .is_err(),
            "unexpected source should be filtered"
        );

        expected_rtp
            .send_to(b"good-rtp", endpoint.local_rtp)
            .expect("send expected rtp");
        expected_rtcp
            .send_to(b"good-rtcp", endpoint.local_rtcp)
            .expect("send expected rtcp");

        let mut saw_rtp = false;
        let mut saw_rtcp = false;
        let res = timeout(Duration::from_secs(2), async {
            while !saw_rtp || !saw_rtcp {
                match event_rx.recv().await {
                    Some(RtspClientEvent::UdpRtp { from, payload, .. }) => {
                        assert_eq!(from, expected_rtp.local_addr().expect("expected rtp addr"));
                        assert_eq!(payload.as_ref(), b"good-rtp");
                        saw_rtp = true;
                    }
                    Some(RtspClientEvent::UdpRtcp { from, payload, .. }) => {
                        assert_eq!(
                            from,
                            expected_rtcp.local_addr().expect("expected rtcp addr")
                        );
                        assert_eq!(payload.as_ref(), b"good-rtcp");
                        saw_rtcp = true;
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await;
        assert!(res.is_ok(), "did not receive expected udp events in time");

        cancel.cancel();
        for join in joins.drain(..) {
            let _ = join.wait().await;
        }
    }
}
