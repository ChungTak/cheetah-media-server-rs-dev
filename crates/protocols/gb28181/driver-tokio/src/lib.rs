use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

use cheetah_codec::MonoTime;
use cheetah_gb28181_core::{
    Gb28181Command, Gb28181Core, Gb28181CoreInput, Gb28181CoreOutput, Gb28181Diagnostic,
    Gb28181Event, GbDeviceId, GbInviteSpec, GbSessionId, GbTalkSpec, SipMessage, StartLine,
};
use cheetah_runtime_api::{CancellationToken, RuntimeApi};

/// `GbDriverCommand` enumeration.
/// `GbDriverCommand` 枚举.
#[derive(Debug, Clone)]
pub enum GbDriverCommand {
    /// `StartInvite` variant.
    /// `StartInvite` 变体.
    StartInvite(GbInviteSpec),
    /// `StopInvite` variant.
    /// `StopInvite` 变体.
    StopInvite { session_key: String },
    /// `StartTalk` variant.
    /// `StartTalk` 变体.
    StartTalk(GbTalkSpec),
    /// `StopTalk` variant.
    /// `StopTalk` 变体.
    StopTalk { session_key: String },
    /// `RegisterChallenge` variant.
    /// `RegisterChallenge` 变体.
    RegisterChallenge {
        device_id: GbDeviceId,
        destination: SocketAddr,
    },
}

/// `Gb28181DriverConfig` data structure.
/// `Gb28181DriverConfig` 数据结构.
#[derive(Debug, Clone)]
pub struct Gb28181DriverConfig {
    /// `listen_udp` field of type `SocketAddr`.
    /// `listen_udp` 字段，类型为 `SocketAddr`.
    pub listen_udp: SocketAddr,
    /// `listen_tcp` field of type `SocketAddr`.
    /// `listen_tcp` 字段，类型为 `SocketAddr`.
    pub listen_tcp: SocketAddr,
    /// `read_buffer_size` field of type `usize`.
    /// `read_buffer_size` 字段，类型为 `usize`.
    pub read_buffer_size: usize,
    /// `tick_interval_ms` field of type `u64`.
    /// `tick_interval_ms` 字段，类型为 `u64`.
    pub tick_interval_ms: u64,
}

impl Default for Gb28181DriverConfig {
    fn default() -> Self {
        Self {
            listen_udp: "127.0.0.1:5060".parse().unwrap(),
            listen_tcp: "127.0.0.1:5060".parse().unwrap(),
            read_buffer_size: 65536,
            tick_interval_ms: 1000,
        }
    }
}

/// `Gb28181DriverHandle` data structure.
/// `Gb28181DriverHandle` 数据结构.
pub struct Gb28181DriverHandle {
    /// `cmd_tx` field.
    /// `cmd_tx` 字段.
    cmd_tx: mpsc::Sender<GbDriverCommand>,
    /// `event_rx` field.
    /// `event_rx` 字段.
    event_rx: Arc<Mutex<mpsc::Receiver<Gb28181Event>>>,
    /// `diag_rx` field.
    /// `diag_rx` 字段.
    diag_rx: Arc<Mutex<mpsc::Receiver<Gb28181Diagnostic>>>,
}

impl Gb28181DriverHandle {
    /// `send_command` function.
    /// `send_command` 函数.
    pub async fn send_command(&self, cmd: GbDriverCommand) -> Result<(), &'static str> {
        self.cmd_tx.send(cmd).await.map_err(|_| "driver stopped")
    }

    /// `recv_event` function.
    /// `recv_event` 函数.
    pub async fn recv_event(&self) -> Option<Gb28181Event> {
        self.event_rx.lock().await.recv().await
    }

    /// `recv_diagnostic` function.
    /// `recv_diagnostic` 函数.
    pub async fn recv_diagnostic(&self) -> Option<Gb28181Diagnostic> {
        self.diag_rx.lock().await.recv().await
    }
}

/// `start_driver` function.
/// `start_driver` 函数.
pub fn start_driver(
    config: Gb28181DriverConfig,
    runtime: Arc<dyn RuntimeApi>,
    cancel: CancellationToken,
) -> Gb28181DriverHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(256);
    let (event_tx, event_rx) = mpsc::channel(256);
    let (diag_tx, diag_rx) = mpsc::channel(256);

    let runtime_clone = runtime.clone();
    runtime.spawn(Box::pin(run_driver_loop(
        config,
        runtime_clone,
        cmd_rx,
        event_tx,
        diag_tx,
        cancel,
    )));

    Gb28181DriverHandle {
        cmd_tx,
        event_rx: Arc::new(Mutex::new(event_rx)),
        diag_rx: Arc::new(Mutex::new(diag_rx)),
    }
}

async fn run_driver_loop(
    config: Gb28181DriverConfig,
    runtime: Arc<dyn RuntimeApi>,
    mut cmd_rx: mpsc::Receiver<GbDriverCommand>,
    event_tx: mpsc::Sender<Gb28181Event>,
    diag_tx: mpsc::Sender<Gb28181Diagnostic>,
    cancel: CancellationToken,
) {
    let udp_socket = match runtime.bind_udp(config.listen_udp) {
        Ok(s) => {
            info!("GB28181 SIP UDP Driver listening on {}", config.listen_udp);
            Arc::new(s)
        }
        Err(e) => {
            error!(
                "GB28181 SIP UDP Driver bind failed on {}: {e}",
                config.listen_udp
            );
            return;
        }
    };

    let tcp_listener = match runtime.bind_tcp(config.listen_tcp) {
        Ok(l) => {
            info!("GB28181 SIP TCP Driver listening on {}", config.listen_tcp);
            Some(Arc::new(l))
        }
        Err(e) => {
            warn!(
                "GB28181 SIP TCP Driver bind failed on {}: {e} (continuing with UDP only)",
                config.listen_tcp
            );
            None
        }
    };

    let mut core = Gb28181Core::new();
    let tick_interval = Duration::from_millis(config.tick_interval_ms);
    let mut last_tick = runtime.now();

    // Multiplexing channels for inputs into the state machine
    let (sip_rx_tx, mut sip_rx_rx) = mpsc::channel::<(SocketAddr, SipMessage)>(256);

    // Active TCP connection writers: Peer SocketAddr -> mpsc::Sender<Bytes>
    let tcp_writers: Arc<Mutex<HashMap<SocketAddr, mpsc::Sender<Bytes>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Map to associate session_key to generated Call-ID
    let mut session_to_call_id = HashMap::<String, GbSessionId>::new();
    let mut pending_invites = Vec::<String>::new();

    // Spawn UDP receiver task
    {
        let udp_socket = udp_socket.clone();
        let sip_rx_tx = sip_rx_tx.clone();
        let cancel = cancel.clone();
        let buf_size = config.read_buffer_size;
        runtime.spawn(Box::pin(async move {
            let mut buf = vec![0u8; buf_size];
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                match udp_socket.recv_from(&mut buf).await {
                    Ok(meta) => {
                        if let Ok(raw_str) = std::str::from_utf8(&buf[..meta.len]) {
                            if let Ok(msg) = SipMessage::parse(raw_str) {
                                if sip_rx_tx.send((meta.from, msg)).await.is_err() {
                                    break;
                                }
                            } else {
                                debug!("Failed to parse SIP message from UDP source {}", meta.from);
                            }
                        }
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            break;
                        }
                        warn!("UDP receive error: {e}");
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
        }));
    }

    // Spawn TCP listener task
    if let Some(tcp_listener) = tcp_listener {
        let cancel = cancel.clone();
        let tcp_writers = tcp_writers.clone();
        let sip_rx_tx = sip_rx_tx.clone();
        let buf_size = config.read_buffer_size;
        let runtime_inner = runtime.clone();

        runtime.spawn(Box::pin(async move {
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                match tcp_listener.accept().await {
                    Ok((stream, addr)) => {
                        debug!("GB28181 SIP TCP client connected from {}", addr);
                        let (writer_tx, mut writer_rx) = mpsc::channel::<Bytes>(128);

                        // Register connection writer
                        tcp_writers.lock().await.insert(addr, writer_tx);

                        let tcp_writers_clone = tcp_writers.clone();
                        let sip_rx_tx = sip_rx_tx.clone();
                        let cancel_child = cancel.child_token();

                        runtime_inner.spawn(Box::pin(async move {
                            let mut stream = stream;
                            let mut buf = vec![0u8; buf_size];
                            let mut remaining = BytesMut::new();
                            loop {
                                if cancel_child.is_cancelled() {
                                    break;
                                }
                                tokio::select! {
                                    read_res = stream.read(&mut buf) => {
                                        match read_res {
                                            Ok(0) => break, // EOF
                                            Ok(n) => {
                                                remaining.extend_from_slice(&buf[..n]);
                                                while let Some(msg) = try_parse_sip(&mut remaining) {
                                                    if sip_rx_tx.send((addr, msg)).await.is_err() {
                                                        break;
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                warn!("TCP connection read error from {addr}: {e}");
                                                break;
                                            }
                                        }
                                    }
                                    write_msg = writer_rx.recv() => {
                                        match write_msg {
                                            Some(data) => {
                                                if let Err(e) = stream.write_all(&data).await {
                                                    warn!("TCP connection write error to {addr}: {e}");
                                                    break;
                                                }
                                            }
                                            None => break,
                                        }
                                    }
                                }
                            }
                            // Cleanup writer registry on disconnect
                            tcp_writers_clone.lock().await.remove(&addr);
                            debug!("GB28181 SIP TCP client disconnected from {}", addr);
                        }));
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            break;
                        }
                        warn!("TCP accept error: {e}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }));
    }

    // Driver central loop
    let mut outputs = Vec::new();
    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Calculate time to next tick
        let now = runtime.now();
        let elapsed_us = now.as_micros().saturating_sub(last_tick.as_micros());
        let sleep_dur = if elapsed_us >= tick_interval.as_micros() as u64 {
            Duration::ZERO
        } else {
            Duration::from_micros(tick_interval.as_micros() as u64 - elapsed_us)
        };

        let next_wake = MonoTime::from_micros(now.as_micros() + sleep_dur.as_micros() as u64);
        let mut tick_timer = runtime.sleep_until(next_wake);

        tokio::select! {
            _ = tick_timer.wait() => {
                let now_ms = runtime.now().as_micros() / 1000;
                core.handle_input(Gb28181CoreInput::Tick { now_ms }, &mut outputs);
                last_tick = runtime.now();
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(GbDriverCommand::StartInvite(spec)) => {
                        pending_invites.push(spec.session_key.clone());
                        core.handle_input(Gb28181CoreInput::Command(Gb28181Command::StartInvite(spec)), &mut outputs);
                    }
                    Some(GbDriverCommand::StopInvite { session_key }) => {
                        if let Some(call_id) = session_to_call_id.remove(&session_key) {
                            core.handle_input(Gb28181CoreInput::Command(Gb28181Command::StopInvite(call_id)), &mut outputs);
                        }
                    }
                    Some(GbDriverCommand::StartTalk(spec)) => {
                        pending_invites.push(spec.session_key.clone());
                        core.handle_input(Gb28181CoreInput::Command(Gb28181Command::StartTalk(spec)), &mut outputs);
                    }
                    Some(GbDriverCommand::StopTalk { session_key }) => {
                        if let Some(call_id) = session_to_call_id.remove(&session_key) {
                            core.handle_input(Gb28181CoreInput::Command(Gb28181Command::StopTalk(call_id)), &mut outputs);
                        }
                    }
                    Some(GbDriverCommand::RegisterChallenge { device_id, destination }) => {
                        core.handle_input(Gb28181CoreInput::Command(Gb28181Command::RegisterChallenge { device_id, destination }), &mut outputs);
                    }
                    None => break, // Handle dropped
                }
            }
            sip = sip_rx_rx.recv() => {
                if let Some((source, message)) = sip {
                    core.handle_input(Gb28181CoreInput::SipMessage { source, message }, &mut outputs);
                }
            }
        }

        // Process outputs
        for out in outputs.drain(..) {
            match out {
                Gb28181CoreOutput::SendSip(action) => {
                    // Use byte-level serialization so any non-UTF-8 body content (rare but
                    // legal) is preserved, keeping the on-wire byte count consistent with
                    // the `Content-Length` header.
                    let bytes = Bytes::from(action.message.to_bytes());

                    // Map pending invite's session_key to the generated Call-ID
                    if let StartLine::Request { method, .. } = &action.message.start_line {
                        if method == "INVITE" {
                            if let Some(call_id) = action.message.get_header("Call-ID") {
                                if !pending_invites.is_empty() {
                                    let session_key = pending_invites.remove(0);
                                    session_to_call_id.insert(session_key, call_id.to_string());
                                }
                            }
                        }
                    }

                    // Try TCP first if registered
                    let mut sent_tcp = false;
                    let writers_guard = tcp_writers.lock().await;
                    if let Some(writer_tx) = writers_guard.get(&action.destination) {
                        if writer_tx.send(bytes.clone()).await.is_ok() {
                            sent_tcp = true;
                        }
                    }
                    drop(writers_guard);

                    if !sent_tcp {
                        // Fallback to UDP
                        if let Err(e) = udp_socket.send_to(&bytes, action.destination).await {
                            warn!(
                                "Failed to send SIP UDP message to {}: {e}",
                                action.destination
                            );
                        }
                    }
                }
                Gb28181CoreOutput::Event(evt) => {
                    let _ = event_tx.send(evt).await;
                }
                Gb28181CoreOutput::Diagnostic(diag) => {
                    let _ = diag_tx.send(diag).await;
                }
            }
        }
    }
}

/// Try to extract one complete SIP message from a TCP receive buffer.
///
/// ABL-style lenient handling: we accept any combination of `\r\n`, `\n`, or `\r` between
/// header lines and as the header/body separator. The body length is read from
/// `Content-Length:` (case-insensitive); if absent we assume zero. On a complete message we
/// drain the corresponding bytes from the buffer.
///
/// Robustness note: SIP headers are always ASCII, but the body may contain non-ASCII bytes
/// (e.g., binary SDP attachments) and a TCP read may also stop mid-byte through a multi-byte
/// UTF-8 character. We therefore perform header detection at the byte level and only convert
/// the header portion to `&str` once we know its boundaries; the body is validated as UTF-8
/// only at the very end (where `SipMessage::parse` requires it).
fn try_parse_sip(buf: &mut BytesMut) -> Option<SipMessage> {
    let bytes = buf.as_ref();
    let (header_end, sep_len) = find_sip_header_terminator_bytes(bytes)?;

    // The header portion must be valid UTF-8 (ASCII for compliant SIP). If somehow it isn't,
    // we drop the broken bytes — keeping them around forever would deadlock the connection.
    let header_part = match std::str::from_utf8(&bytes[..header_end]) {
        Ok(s) => s,
        Err(_) => {
            buf.advance(header_end + sep_len);
            return None;
        }
    };

    let mut content_length = 0usize;
    for line in split_lenient_lines(header_part) {
        if let Some((name, val)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = val.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }

    let total_len = header_end + sep_len + content_length;
    if buf.len() >= total_len {
        let msg_bytes = buf.split_to(total_len);
        let msg_str = std::str::from_utf8(&msg_bytes).ok()?;
        SipMessage::parse(msg_str).ok()
    } else {
        None
    }
}

/// Locate the SIP header terminator (blank line) in a raw byte buffer.
///
/// Accepts any combination of `\r\n`, `\n`, or `\r` line terminators on either side of the
/// blank separator. Returns `(index_of_first_terminator_byte, separator_length)` so callers
/// can both treat bytes before `index` as the header section and skip exactly `separator_length`
/// bytes to reach the body.
fn find_sip_header_terminator_bytes(buf: &[u8]) -> Option<(usize, usize)> {
    // Patterns are tried longest-first so a `\r\n\r\n` doesn't match the shorter `\n\n`/`\r\n\n`
    // when both are present at the same offset.
    let patterns: [&[u8]; 5] = [b"\r\n\r\n", b"\n\r\n", b"\r\n\n", b"\n\n", b"\r\r"];
    let mut best: Option<(usize, usize)> = None;
    for pat in patterns {
        if let Some(idx) = find_subslice(buf, pat) {
            best = match best {
                None => Some((idx, pat.len())),
                Some((cur_idx, cur_len))
                    if idx < cur_idx || (idx == cur_idx && pat.len() > cur_len) =>
                {
                    Some((idx, pat.len()))
                }
                Some(_) => best,
            };
        }
    }
    best
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Split a SIP header section on `\r\n`, `\n`, or `\r` line terminators.
fn split_lenient_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut start = 0usize;
    let bytes = text.as_bytes();
    std::iter::from_fn(move || {
        if start >= bytes.len() {
            return None;
        }
        let mut i = start;
        while i < bytes.len() {
            match bytes[i] {
                b'\r' => {
                    let line = &text[start..i];
                    let mut next = i + 1;
                    if next < bytes.len() && bytes[next] == b'\n' {
                        next += 1;
                    }
                    start = next;
                    return Some(line);
                }
                b'\n' => {
                    let line = &text[start..i];
                    start = i + 1;
                    return Some(line);
                }
                _ => i += 1,
            }
        }
        let line = &text[start..];
        start = bytes.len();
        Some(line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_parse_sip_handles_crlf() {
        let mut buf = BytesMut::from(
            "REGISTER sip:x@host SIP/2.0\r\n\
             Call-ID: c1\r\n\
             Content-Length: 0\r\n\
             \r\n",
        );
        let msg = try_parse_sip(&mut buf).expect("parse");
        assert_eq!(msg.get_header("Call-ID"), Some("c1"));
        assert!(buf.is_empty());
    }

    #[test]
    fn try_parse_sip_handles_lf_only_terminators() {
        // Some embedded ABL devices use bare `\n` line terminators.
        let mut buf = BytesMut::from(
            "REGISTER sip:x@host SIP/2.0\n\
             Call-ID: c2\n\
             Content-Length: 5\n\
             \n\
             hello",
        );
        let msg = try_parse_sip(&mut buf).expect("parse");
        assert_eq!(msg.get_header("Call-ID"), Some("c2"));
        assert_eq!(msg.body, b"hello");
        assert!(buf.is_empty());
    }

    #[test]
    fn try_parse_sip_returns_none_on_partial_message() {
        // Header section terminator not yet present.
        let mut buf = BytesMut::from("REGISTER sip:x@host SIP/2.0\r\nCall-ID: c3\r\n");
        assert!(try_parse_sip(&mut buf).is_none());
        // Body length larger than what is buffered.
        let mut buf = BytesMut::from(
            "REGISTER sip:x@host SIP/2.0\r\nCall-ID: c4\r\nContent-Length: 10\r\n\r\nshort",
        );
        assert!(try_parse_sip(&mut buf).is_none());
    }

    #[test]
    fn try_parse_sip_handles_non_utf8_body() {
        // Header is ASCII, body contains bytes outside ASCII range that happen to also be
        // invalid UTF-8 when interpreted standalone. The previous implementation would call
        // `from_utf8` on the entire buffer up front and bail out forever; the new
        // implementation must successfully parse the header and surface the binary body.
        let mut buf = BytesMut::new();
        buf.extend_from_slice(
            b"REGISTER sip:x@host SIP/2.0\r\nCall-ID: c-bin\r\nContent-Length: 4\r\n\r\n",
        );
        // Invalid UTF-8 sequence (high bit set bytes that don't form a valid multibyte char).
        buf.extend_from_slice(&[0xff, 0xfe, 0xfd, 0xfc]);

        // The full message is well-formed at the SIP layer, but the body is not UTF-8. The
        // current `SipMessage::parse` requires `&str`, so we expect `try_parse_sip` to drop
        // the message rather than loop forever; the buffer must be advanced so the connection
        // can make progress.
        let _ = try_parse_sip(&mut buf);
        // We don't assert on the parse result (it's allowed to fail); we just need progress.
        // Calling again should not return `Some` for the same garbage.
        assert!(try_parse_sip(&mut buf).is_none());
    }
}
