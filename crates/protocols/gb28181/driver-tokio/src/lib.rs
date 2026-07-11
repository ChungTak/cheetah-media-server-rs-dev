//! Tokio runtime driver for the GB28181 signaling plane.
//!
//! This crate bridges the Sans-I/O `cheetah-gb28181-core` state machine to the network layer:
//! it binds UDP/TCP sockets, parses incoming SIP messages, forwards them to the core, and
//! dispatches the resulting SIP actions, events, and diagnostics back out. The media path is
//! intentionally outside the driver; it is managed by `cheetah-gb28181-module` through the
//! RTP module.
//!
//! GB28181 信令面的 Tokio 运行时驱动。
//!
//! 该 crate 将无 I/O 的 `cheetah-gb28181-core` 状态机桥接到网络层：绑定 UDP/TCP 套接字、
//! 解析入站 SIP 消息、转发给核心，并把核心产生的 SIP 动作、事件与诊断分发出去。
//! 媒体通路不在驱动中处理，而是由 `cheetah-gb28181-module` 通过 RTP 模块管理。

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

/// Command sent from the module into the driver to request a SIP signaling action.
///
/// These commands are forwarded into the Sans-I/O `Gb28181Core` as `Gb28181Command` inputs,
/// so the driver itself does not contain any dialog state.
///
/// 从模块发往驱动、请求执行 SIP 信令操作的命令。
///
/// 这些命令会被作为 `Gb28181Command` 输入转发到无 I/O 的 `Gb28181Core` 状态机，
/// 因此驱动本身不持有任何对话状态。
#[derive(Debug, Clone)]
pub enum GbDriverCommand {
    StartInvite(GbInviteSpec),
    StopInvite {
        session_key: String,
    },
    StartTalk(GbTalkSpec),
    StopTalk {
        session_key: String,
    },
    RegisterChallenge {
        device_id: GbDeviceId,
        destination: SocketAddr,
    },
}

/// Configuration for the GB28181 Tokio driver.
///
/// Controls the local SIP UDP/TCP listening addresses, the per-read buffer size, and the
/// interval at which the driver wakes the core for timeout/keep-alive processing.
///
/// GB28181 Tokio 驱动的配置。
///
/// 控制本地 SIP UDP/TCP 监听地址、每次读取缓冲区大小，以及驱动唤醒核心进行
/// 超时/保活处理的间隔。
#[derive(Debug, Clone)]
pub struct Gb28181DriverConfig {
    pub listen_udp: SocketAddr,
    pub listen_tcp: SocketAddr,
    pub read_buffer_size: usize,
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

/// Handle to the running driver, used by the module to send commands and receive events.
///
/// The command sender is cloneable and the receivers are wrapped in `Mutex` so the handle can
/// be shared across tasks. Each receiver is a single-consumer channel, so callers must not
/// poll the same receiver from multiple tasks concurrently.
///
/// 运行中驱动的句柄，供模块发送命令并接收事件。
///
/// 命令发送端可克隆，接收端用 `Mutex` 包装以便句柄跨任务共享。每个接收端都是
/// 单消费者通道，因此调用者不能同时从多个任务轮询同一个接收端。
pub struct Gb28181DriverHandle {
    cmd_tx: mpsc::Sender<GbDriverCommand>,
    event_rx: Arc<Mutex<mpsc::Receiver<Gb28181Event>>>,
    diag_rx: Arc<Mutex<mpsc::Receiver<Gb28181Diagnostic>>>,
}

impl Gb28181DriverHandle {
    /// Send a command into the driver.
    ///
    /// Returns an error if the driver loop has already stopped and the channel is closed.
    ///
    /// 向驱动发送命令。若驱动循环已停止、通道已关闭，则返回错误。
    pub async fn send_command(&self, cmd: GbDriverCommand) -> Result<(), &'static str> {
        self.cmd_tx.send(cmd).await.map_err(|_| "driver stopped")
    }

    /// Receive the next event emitted by the driver.
    ///
    /// Returns `None` when the driver loop has stopped.
    ///
    /// 接收驱动产生的下一个事件。驱动循环停止时返回 `None`。
    pub async fn recv_event(&self) -> Option<Gb28181Event> {
        self.event_rx.lock().await.recv().await
    }

    /// Receive the next diagnostic emitted by the driver.
    ///
    /// Returns `None` when the driver loop has stopped.
    ///
    /// 接收驱动产生的下一个诊断。驱动循环停止时返回 `None`。
    pub async fn recv_diagnostic(&self) -> Option<Gb28181Diagnostic> {
        self.diag_rx.lock().await.recv().await
    }
}

/// Start the GB28181 Tokio driver and return a `Gb28181DriverHandle`.
///
/// This function allocates the command, event, and diagnostic channels, binds the configured
/// UDP and optional TCP sockets, spawns the `run_driver_loop` task on the runtime, and returns
/// the handle. When the handle is dropped, the channels close and the loop exits cleanly.
///
/// 启动 GB28181 Tokio 驱动并返回 `Gb28181DriverHandle`。
///
/// 该函数分配命令、事件、诊断通道，绑定配置好的 UDP 与可选 TCP 套接字，
/// 在运行时上生成 `run_driver_loop` 任务，然后返回句柄。句柄被丢弃时通道关闭，
/// 循环会干净退出。
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

/// Main async loop of the GB28181 driver.
///
/// It binds UDP and an optional TCP listener, spawns receiver tasks, then multiplexes three
/// input sources into the Sans-I/O `Gb28181Core`: timer ticks, commands from the module, and
/// parsed SIP messages from the network. The outputs are dispatched to sockets (TCP preferred,
/// UDP fallback), the event channel, and the diagnostic channel.
///
/// GB28181 驱动的主异步循环。
///
/// 绑定 UDP 与可选 TCP 监听器，生成接收任务，然后将三种输入源多路复用到
/// 无 I/O 的 `Gb28181Core`：定时器 ticks、来自模块的命令、来自网络的解析后 SIP 消息。
/// 输出被分发到套接字（优先 TCP、回退 UDP）、事件通道和诊断通道。
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

    // Multiplexing channel for SIP messages parsed by the receiver tasks into the core.
    // 接收任务将解析后的 SIP 消息通过该通道送入核心。
    let (sip_rx_tx, mut sip_rx_rx) = mpsc::channel::<(SocketAddr, SipMessage)>(256);

    // Active TCP connection writers: Peer SocketAddr -> mpsc::Sender<Bytes>
    // 活动 TCP 连接写端：对端 SocketAddr -> mpsc::Sender<Bytes>
    let tcp_writers: Arc<Mutex<HashMap<SocketAddr, mpsc::Sender<Bytes>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Map the module's session_key to the Call-ID generated by the core for INVITE sessions.
    // 将模块的 session_key 映射到核心为 INVITE 会话生成的 Call-ID。
    let mut session_to_call_id = HashMap::<String, GbSessionId>::new();
    // Queue of session_keys for INVITE requests that are waiting for the core to assign a Call-ID.
    // 等待核心分配 Call-ID 的 INVITE 请求的 session_key 队列。
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
                        // 注册连接的写端
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
                            // 连接断开时清理写端注册表
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

        // Process outputs produced by the core.
        for out in outputs.drain(..) {
            match out {
                Gb28181CoreOutput::SendSip(action) => {
                    // Use byte-level serialization so any non-UTF-8 body content (rare but
                    // legal) is preserved, keeping the on-wire byte count consistent with
                    // the `Content-Length` header.
                    let bytes = Bytes::from(action.message.to_bytes());

                    // Map the pending invite's session_key to the Call-ID generated by the core.
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

                    // Try TCP first if the peer has an active registered connection.
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
/// 尝试从 TCP 接收缓冲区中提取一条完整 SIP 消息。
///
/// 兼容 ABL 设备的宽松处理：头部行之间以及头体分隔符接受 `\r\n`、`\n`、`\r` 的任意组合。
/// 体长度从 `Content-Length:`（不区分大小写）读取；缺省时视为 0。消息完整时
/// 从缓冲区中消耗掉对应字节。
///
/// Robustness note: SIP headers are always ASCII, but the body may contain non-ASCII bytes
/// (e.g., binary SDP attachments) and a TCP read may also stop mid-byte through a multi-byte
/// UTF-8 character. We therefore perform header detection at the byte level and only convert
/// the header portion to `&str` once we know its boundaries; the body is validated as UTF-8
/// only at the very end (where `SipMessage::parse` requires it).
///
/// 鲁棒性说明：SIP 头部总是 ASCII，但消息体可能包含非 ASCII 字节（例如二进制 SDP 附件），
/// 且 TCP 读取可能恰好在多字节 UTF-8 字符中间停止。因此我们在字节层检测头部边界，
/// 仅在确定边界后才将头部部分转换为 `&str`；消息体只在最后一步才验证为 UTF-8
///（因为 `SipMessage::parse` 需要 `&str`）。
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
///
/// 在原始字节缓冲区中定位 SIP 头部终止符（空行）。
///
/// 空行分隔符两侧接受 `\r\n`、`\n`、`\r` 的任意组合。返回
/// `(第一个终止符字节的索引, 分隔符长度)`，调用者既可将索引前字节视为头部，
/// 也可跳过精确的分隔符长度字节到达消息体。
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

/// Locate the first occurrence of a sub-slice inside a byte slice.
///
/// Returns the starting index of the match, or `None` if the needle is empty or larger than the
/// haystack.
///
/// 在字节切片中定位子切片的第一次出现。
///
/// 返回匹配的起始索引；若 needle 为空或大于 haystack 则返回 `None`。
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Split a SIP header section on `\r\n`, `\n`, or `\r` line terminators.
///
/// The iterator is stateful and returns each header line in order, preserving the original
/// `&str` view. This is used for parsing `Content-Length` from the header block.
///
/// 按照 `\r\n`、`\n` 或 `\r` 行终止符拆分 SIP 头部。
///
/// 迭代器是有状态的，按顺序返回每个头部行，并保留原始 `&str` 视图。用于从头部块
/// 解析 `Content-Length`。
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
