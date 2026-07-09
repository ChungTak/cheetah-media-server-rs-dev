use super::*;
use cheetah_sdk::AsyncUdpSocket;
use std::net::IpAddr;

pub(super) const MAX_UDP_PORT_PAIR_BIND_ATTEMPTS: usize = 256;
pub(super) type UdpSocketPair = (Arc<dyn AsyncUdpSocket>, Arc<dyn AsyncUdpSocket>, u16, u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UdpSocketPairBindError {
    PoolExhausted,
    BindFailure,
}

pub(super) fn bind_udp_socket_pair(
    runtime_api: &Arc<dyn RuntimeApi>,
    bind_addr: SocketAddr,
    pool_start: u16,
    pool_end: u16,
    max_attempts: usize,
) -> Result<UdpSocketPair, UdpSocketPairBindError> {
    if max_attempts == 0 {
        return Err(UdpSocketPairBindError::BindFailure);
    }
    if validate_udp_server_port_pool_range(pool_start, pool_end).is_err() {
        return Err(UdpSocketPairBindError::BindFailure);
    }
    let first_even = if pool_start.is_multiple_of(2) {
        pool_start
    } else {
        pool_start.saturating_add(1)
    };
    let mut pairs = Vec::new();
    let mut port = first_even;
    while port < pool_end {
        pairs.push(port);
        if pool_end.saturating_sub(port) < 2 {
            break;
        }
        port = port.saturating_add(2);
    }
    if pairs.is_empty() {
        return Err(UdpSocketPairBindError::PoolExhausted);
    }

    let max_tries = max_attempts.min(pairs.len());
    let start_idx = usize::from(bind_addr.port()) % pairs.len();
    let bind_ip = bind_addr.ip();
    let mut saw_bind_failure = false;
    for offset in 0..max_tries {
        let pair_idx = (start_idx + offset) % pairs.len();
        let rtp_port = pairs[pair_idx];
        let rtp_addr = SocketAddr::new(bind_ip, rtp_port);
        let rtcp_addr = SocketAddr::new(bind_ip, rtp_port.saturating_add(1));

        let rtp_socket: Arc<dyn AsyncUdpSocket> = match runtime_api.bind_udp(rtp_addr) {
            Ok(socket) => Arc::from(socket),
            Err(_) => {
                saw_bind_failure = true;
                continue;
            }
        };
        let rtcp_socket: Arc<dyn AsyncUdpSocket> = match runtime_api.bind_udp(rtcp_addr) {
            Ok(socket) => Arc::from(socket),
            Err(_) => {
                saw_bind_failure = true;
                continue;
            }
        };
        if validate_udp_server_port_pair_shape(rtp_port, rtp_port.saturating_add(1)).is_ok() {
            return Ok((
                rtp_socket,
                rtcp_socket,
                rtp_port,
                rtp_port.saturating_add(1),
            ));
        }
        return Err(UdpSocketPairBindError::BindFailure);
    }

    if saw_bind_failure {
        return Err(UdpSocketPairBindError::PoolExhausted);
    }
    Err(UdpSocketPairBindError::BindFailure)
}

pub(super) async fn send_udp_hole_punch_probe(
    rtp_socket: &Arc<dyn AsyncUdpSocket>,
    rtcp_socket: &Arc<dyn AsyncUdpSocket>,
    target_rtp: SocketAddr,
    target_rtcp: SocketAddr,
) -> Result<(), &'static [u8]> {
    const PROBE_PACKET: [u8; 1] = [0u8];
    if rtp_socket.send_to(&PROBE_PACKET, target_rtp).await.is_err() {
        return Err(b"send rtp udp hole punch probe failed");
    }
    if rtcp_socket
        .send_to(&PROBE_PACKET, target_rtcp)
        .await
        .is_err()
    {
        return Err(b"send rtcp udp hole punch probe failed");
    }
    Ok(())
}

pub(super) fn validate_udp_server_port_pair_shape(
    rtp_port: u16,
    rtcp_port: u16,
) -> Result<(), &'static [u8]> {
    if is_even_odd_port_pair(rtp_port, rtcp_port) {
        Ok(())
    } else {
        Err(b"invalid udp server port pair")
    }
}

pub(super) fn validate_udp_server_port_pool_range(
    pool_start: u16,
    pool_end: u16,
) -> Result<(), &'static [u8]> {
    let first_even = if pool_start.is_multiple_of(2) {
        pool_start
    } else {
        pool_start.saturating_add(1)
    };
    if first_even.saturating_add(1) > pool_end {
        Err(b"udp server port pool has no valid even/odd pair")
    } else {
        Ok(())
    }
}

pub(super) fn resolve_udp_destination_ip(
    peer: SocketAddr,
    destination: Option<IpAddr>,
) -> Result<IpAddr, &'static [u8]> {
    match destination {
        None => Ok(peer.ip()),
        Some(ip) if ip == peer.ip() => Ok(ip),
        Some(_) => Err(b"third-party destination is not allowed"),
    }
}

fn is_even_odd_port_pair(rtp_port: u16, rtcp_port: u16) -> bool {
    rtp_port.is_multiple_of(2) && rtcp_port == rtp_port.saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_udp_destination_only_allows_peer_ip_by_default() {
        let peer = SocketAddr::from(([10, 0, 0, 7], 554));
        let resolved = resolve_udp_destination_ip(peer, None).expect("fallback peer");
        assert_eq!(resolved, peer.ip());

        let same = resolve_udp_destination_ip(peer, Some(peer.ip())).expect("same ip");
        assert_eq!(same, peer.ip());

        let rejected = resolve_udp_destination_ip(
            peer,
            Some(IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 8))),
        );
        assert_eq!(
            rejected,
            Err(b"third-party destination is not allowed".as_slice())
        );
    }

    #[test]
    fn port_pair_shape_requires_even_rtp_followed_by_rtcp_plus_one() {
        assert!(super::is_even_odd_port_pair(62000, 62001));
        assert!(!super::is_even_odd_port_pair(62001, 62002));
        assert!(!super::is_even_odd_port_pair(62000, 62002));
    }

    #[test]
    fn pool_range_without_even_odd_pair_is_rejected() {
        assert_eq!(
            validate_udp_server_port_pool_range(62001, 62001),
            Err(b"udp server port pool has no valid even/odd pair".as_slice())
        );
        assert_eq!(
            validate_udp_server_port_pool_range(62001, 62002),
            Err(b"udp server port pool has no valid even/odd pair".as_slice())
        );
        assert_eq!(validate_udp_server_port_pool_range(62000, 62001), Ok(()));
    }
}
