#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bytes::Bytes;
use cheetah_codec::{RtpHeader, RtpPacket};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::{sleep, timeout};

pub(crate) const ANNOUNCE_SDP: &str = "v=0\r\n\
o=- 0 0 IN IP4 127.0.0.1\r\n\
s=No Name\r\n\
t=0 0\r\n\
m=video 0 RTP/AVP 96\r\n\
a=rtpmap:96 H264/90000\r\n\
a=fmtp:96 packetization-mode=1;sprop-parameter-sets=Z0IAH5WoFAFuQA==,aM4G4g==\r\n\
a=control:trackID=0\r\n\
m=audio 0 RTP/AVP 97\r\n\
a=rtpmap:97 MPEG4-GENERIC/48000/2\r\n\
a=fmtp:97 profile-level-id=1;mode=AAC-hbr;config=1190\r\n\
a=control:trackID=1\r\n";

#[derive(Debug)]
pub(crate) struct RtspResponse {
    pub(crate) status_code: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl RtspResponse {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub(crate) async fn connect_with_retry(addr: SocketAddr) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match TcpStream::connect(addr).await {
            Ok(stream) => return stream,
            Err(err) => {
                if Instant::now() >= deadline {
                    panic!("connect to rtsp listener timeout: {err}");
                }
                sleep(Duration::from_millis(20)).await;
            }
        }
    }
}

pub(crate) async fn write_request(stream: &mut TcpStream, request: &str) {
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
}

pub(crate) fn build_request(
    method: &str,
    uri: &str,
    cseq: u32,
    session: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: &[u8],
) -> String {
    let mut request = format!("{method} {uri} RTSP/1.0\r\nCSeq: {cseq}\r\n");
    if let Some(session) = session {
        request.push_str(&format!("Session: {session}\r\n"));
    }
    for (name, value) in extra_headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    request.push_str(std::str::from_utf8(body).expect("request body utf-8"));
    request
}

pub(crate) async fn read_response(stream: &mut TcpStream, stage: &str) -> RtspResponse {
    let mut buf = Vec::<u8>::new();
    loop {
        if buf.is_empty() {
            read_one_byte(stream, &mut buf, stage).await;
        }
        if buf[0] == b'$' {
            while buf.len() < 4 {
                read_one_byte(stream, &mut buf, stage).await;
            }
            let payload_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
            let total_len = 4 + payload_len;
            while buf.len() < total_len {
                read_one_byte(stream, &mut buf, stage).await;
            }
            buf.drain(..total_len);
            continue;
        }

        let header_end = loop {
            if let Some(header_end) = find_header_end(&buf) {
                break header_end;
            }
            read_one_byte(stream, &mut buf, stage).await;
        };
        let header_text = std::str::from_utf8(&buf[..header_end])
            .expect("response header utf-8")
            .to_string();
        let content_length = parse_content_length(&header_text);
        let total_len = header_end + 4 + content_length;
        while buf.len() < total_len {
            read_one_byte(stream, &mut buf, stage).await;
        }

        let mut lines = header_text.split("\r\n");
        let status_line = lines.next().expect("status line");
        let status_code = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse::<u16>().ok())
            .expect("status code");

        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if let Some((name, value)) = line.split_once(':') {
                headers.push((name.trim().to_string(), value.trim().to_string()));
            }
        }
        let body = buf[header_end + 4..total_len].to_vec();
        return RtspResponse {
            status_code,
            headers,
            body,
        };
    }
}

async fn read_one_byte(stream: &mut TcpStream, buf: &mut Vec<u8>, stage: &str) {
    let mut one = [0u8; 1];
    match timeout(Duration::from_secs(1), stream.read_exact(&mut one)).await {
        Ok(Ok(_)) => buf.push(one[0]),
        Ok(Err(err)) => panic!("read response byte failed at {stage}: {err}"),
        Err(_) => panic!("read response byte timeout at {stage}"),
    }
}

pub(crate) fn find_header_end(data: &[u8]) -> Option<usize> {
    if data.len() < 4 {
        return None;
    }
    data.windows(4).position(|window| window == b"\r\n\r\n")
}

pub(crate) fn parse_content_length(header_text: &str) -> usize {
    for line in header_text.split("\r\n") {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                return value.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }
    0
}

pub(crate) fn parse_transport_server_ports(transport: &str) -> Option<(u16, u16)> {
    for part in transport.split(';') {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("server_port") {
            let (rtp, rtcp) = value.trim().split_once('-')?;
            let rtp = rtp.parse::<u16>().ok()?;
            let rtcp = rtcp.parse::<u16>().ok()?;
            return Some((rtp, rtcp));
        }
    }
    None
}

pub(crate) fn parse_transport_ssrc(transport: &str) -> Option<u32> {
    for part in transport.split(';') {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("ssrc") {
            return u32::from_str_radix(value.trim(), 16).ok();
        }
    }
    None
}

pub(crate) async fn receive_udp_play_rtp_with_publish_retry(
    publisher_rtp: &UdpSocket,
    publish_server_rtp_port: u16,
    client_rtp: &UdpSocket,
    play_server_rtp_port: u16,
) -> RtpPacket {
    let mut recv_buf = [0u8; 2048];
    for attempt in 0..6u16 {
        let seq = 1000u16.wrapping_add(attempt);
        let timestamp = 90_000u32.wrapping_add(u32::from(attempt) * 3_600u32);
        send_publish_udp_rtp_frame(
            publisher_rtp,
            publish_server_rtp_port,
            seq,
            timestamp,
            0x1122_3344,
        )
        .await;

        match timeout(
            Duration::from_millis(350),
            client_rtp.recv_from(&mut recv_buf),
        )
        .await
        {
            Ok(Ok((n, from))) => {
                assert_eq!(from.port(), play_server_rtp_port);
                if let Some(pkt) = RtpPacket::parse(&recv_buf[..n]) {
                    return pkt;
                }
            }
            Ok(Err(err)) => panic!("recv udp rtp failed: {err}"),
            Err(_) => {}
        }
    }
    panic!("did not receive forwarded udp rtp");
}

pub(crate) async fn send_publish_udp_rtp_frame(
    publisher_rtp: &UdpSocket,
    publish_server_rtp_port: u16,
    sequence_number: u16,
    timestamp: u32,
    ssrc: u32,
) {
    let publish_target = SocketAddr::from(([127, 0, 0, 1], publish_server_rtp_port));
    let publish_rtp = build_publish_h264_rtp(sequence_number, timestamp, ssrc);
    publisher_rtp
        .send_to(&publish_rtp, publish_target)
        .await
        .expect("send publisher udp rtp");
}

pub(crate) async fn send_publish_udp_rtcp_sr(
    publisher_rtcp: &UdpSocket,
    publish_server_rtcp_port: u16,
    sender_ssrc: u32,
    rtp_timestamp: u32,
    packet_count: u32,
    octet_count: u32,
) {
    let publish_target = SocketAddr::from(([127, 0, 0, 1], publish_server_rtcp_port));
    let sr_payload =
        build_rtcp_sender_report_packet(sender_ssrc, rtp_timestamp, packet_count, octet_count);
    publisher_rtcp
        .send_to(&sr_payload, publish_target)
        .await
        .expect("send publisher udp rtcp sr");
}

pub(crate) async fn send_publish_sr_and_receive_rr_with_retry(
    publisher_rtcp: &UdpSocket,
    publish_server_rtcp_port: u16,
    publish_sr_ssrc: u32,
) -> (SocketAddr, Vec<u8>) {
    let mut recv_buf = [0u8; 2048];
    let publish_target = SocketAddr::from(([127, 0, 0, 1], publish_server_rtcp_port));
    for attempt in 0..6u32 {
        let rtp_timestamp = 180_000u32.wrapping_add(attempt * 9_000u32);
        let packet_count = 1u32.wrapping_add(attempt);
        let octet_count = 1200u32.wrapping_add(attempt * 120u32);
        let sr_payload = build_rtcp_sender_report_packet(
            publish_sr_ssrc,
            rtp_timestamp,
            packet_count,
            octet_count,
        );
        publisher_rtcp
            .send_to(&sr_payload, publish_target)
            .await
            .expect("send publisher udp rtcp sr");
        match timeout(
            Duration::from_millis(350),
            publisher_rtcp.recv_from(&mut recv_buf),
        )
        .await
        {
            Ok(Ok((n, from))) => {
                return (from, recv_buf[..n].to_vec());
            }
            Ok(Err(err)) => panic!("recv publisher udp rtcp rr failed: {err}"),
            Err(_) => {}
        }
    }
    panic!("did not receive publisher udp rtcp rr");
}

pub(crate) async fn receive_play_udp_rtcp_packets_with_retry(
    publisher_rtp: &UdpSocket,
    publish_server_rtp_port: u16,
    client_rtcp: &UdpSocket,
    play_server_rtcp_port: u16,
) -> (Vec<u8>, Vec<u32>) {
    let mut types = Vec::new();
    let mut sr_sender_ssrcs = Vec::new();
    let mut recv_buf = [0u8; 2048];
    let publish_target = SocketAddr::from(([127, 0, 0, 1], publish_server_rtp_port));
    for attempt in 0..6u16 {
        let seq = 3000u16.wrapping_add(attempt);
        let timestamp = 360_000u32.wrapping_add(u32::from(attempt) * 3_600u32);
        let publish_rtp = build_publish_h264_rtp(seq, timestamp, 0x2233_4455);
        publisher_rtp
            .send_to(&publish_rtp, publish_target)
            .await
            .expect("send publisher udp rtp for play rtcp");

        let end = Instant::now() + Duration::from_millis(350);
        loop {
            let now = Instant::now();
            if now >= end {
                break;
            }
            let left = end.saturating_duration_since(now);
            match timeout(left, client_rtcp.recv_from(&mut recv_buf)).await {
                Ok(Ok((n, from))) => {
                    if from.port() != play_server_rtcp_port || n < 8 {
                        continue;
                    }
                    let packet_type = recv_buf[1];
                    types.push(packet_type);
                    if packet_type == 200 {
                        if let Some(sender_ssrc) = read_u32_be(&recv_buf[4..8]) {
                            sr_sender_ssrcs.push(sender_ssrc);
                        }
                    }
                    if types.contains(&200) && types.contains(&202) {
                        return (types, sr_sender_ssrcs);
                    }
                }
                Ok(Err(err)) => panic!("recv player udp rtcp failed: {err}"),
                Err(_) => break,
            }
        }
    }
    panic!("did not receive both play udp rtcp SDES and SR");
}

pub(crate) async fn wait_for_rtcp_packet_type(
    sock: &UdpSocket,
    expected_from_port: u16,
    expected_type: u8,
) -> (SocketAddr, Vec<u8>) {
    let mut recv_buf = [0u8; 2048];
    let deadline = Instant::now() + Duration::from_millis(600);
    loop {
        let now = Instant::now();
        assert!(
            now < deadline,
            "did not receive RTCP packet type {expected_type} before timeout"
        );
        let left = deadline.saturating_duration_since(now);
        let (n, from) = timeout(left, sock.recv_from(&mut recv_buf))
            .await
            .expect("wait rtcp packet timeout")
            .expect("recv rtcp packet failed");
        if from.port() != expected_from_port || n < 2 {
            continue;
        }
        if recv_buf[1] == expected_type {
            return (from, recv_buf[..n].to_vec());
        }
    }
}

pub(crate) async fn drain_udp_socket(sock: &UdpSocket) {
    let mut recv_buf = [0u8; 2048];
    for _ in 0..32 {
        match timeout(Duration::from_millis(10), sock.recv_from(&mut recv_buf)).await {
            Ok(Ok((_n, _from))) => {}
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
}

pub(crate) async fn assert_no_udp_packet_for_duration(
    sock: &UdpSocket,
    wait: Duration,
    stage: &str,
) {
    let mut recv_buf = [0u8; 2048];
    match timeout(wait, sock.recv_from(&mut recv_buf)).await {
        Ok(Ok((n, from))) => {
            panic!("unexpected udp packet while {stage}: {n} bytes from {from}");
        }
        Ok(Err(err)) => {
            panic!("recv udp packet failed while {stage}: {err}");
        }
        Err(_) => {}
    }
}

pub(crate) fn build_publish_h264_rtp(sequence_number: u16, timestamp: u32, ssrc: u32) -> Bytes {
    let pkt = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number,
            timestamp,
            ssrc,
            marker: true,
        },
        payload: Bytes::from_static(&[0x65, 0x88, 0x84, 0x21]),
    };
    pkt.encode()
}

pub(crate) async fn send_interleaved_frame(stream: &mut TcpStream, channel: u8, payload: &[u8]) {
    let len = payload.len().min(u16::MAX as usize);
    let mut packet = Vec::with_capacity(4 + len);
    packet.push(b'$');
    packet.push(channel);
    packet.extend_from_slice(&(len as u16).to_be_bytes());
    packet.extend_from_slice(&payload[..len]);
    stream
        .write_all(&packet)
        .await
        .expect("write interleaved frame");
}

pub(crate) async fn read_interleaved_frame(stream: &mut TcpStream, stage: &str) -> (u8, Vec<u8>) {
    let mut header = [0u8; 4];
    match timeout(Duration::from_secs(1), stream.read_exact(&mut header)).await {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => panic!("read interleaved header failed at {stage}: {err}"),
        Err(_) => panic!("read interleaved header timeout at {stage}"),
    }

    assert_eq!(
        header[0], b'$',
        "expected interleaved frame start '$' at {stage}, got {:02x}",
        header[0]
    );

    let channel = header[1];
    let payload_len = u16::from_be_bytes([header[2], header[3]]) as usize;
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        match timeout(Duration::from_secs(1), stream.read_exact(&mut payload)).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => panic!("read interleaved payload failed at {stage}: {err}"),
            Err(_) => panic!("read interleaved payload timeout at {stage}"),
        }
    }
    (channel, payload)
}

pub(crate) fn build_rtcp_sender_report_packet(
    sender_ssrc: u32,
    rtp_timestamp: u32,
    packet_count: u32,
    octet_count: u32,
) -> Bytes {
    let mut out = Vec::with_capacity(28);
    out.extend_from_slice(&[0x80, 200, 0x00, 0x06]);
    out.extend_from_slice(&sender_ssrc.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&rtp_timestamp.to_be_bytes());
    out.extend_from_slice(&packet_count.to_be_bytes());
    out.extend_from_slice(&octet_count.to_be_bytes());
    Bytes::from(out)
}

pub(crate) fn read_u32_be(raw: &[u8]) -> Option<u32> {
    if raw.len() < 4 {
        return None;
    }
    Some(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))
}
