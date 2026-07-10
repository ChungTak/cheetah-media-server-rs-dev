use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use bytes::Bytes;

use crate::amf::AmfValue;
use crate::bytes::Buf;
use crate::chunk::RtmpChunkDecoder;
use crate::chunk::RtmpChunkSize;
use crate::error::{Error as RtmpProtocolError, ErrorKind};
use crate::handshake::RtmpServerHandshake;
use crate::message::{decode_rtmp_chunk_to_message, RtmpMessageEncoder, RtmpMessageType};

mod command;
mod handshake;
mod media;

/// Identifier for timers set or cancelled through the core output.
/// 通过核心输出设置或取消的定时器标识符。
pub type TimerId = u64;

/// Input event driving the Sans-I/O RTMP core state machine.
/// 驱动无 I/O RTMP 核心状态机的输入事件。
#[derive(Debug, Clone)]
pub enum CoreInput {
    /// Raw bytes received from the peer.
    /// 从对端收到的原始字节。
    Bytes(Bytes),
    /// A previously set timer has fired.
    /// 先前设置的定时器已触发。
    Timeout { id: TimerId },
    /// An external command to the core, such as accepting a publish request.
    /// 外部给核心的命令，例如接受发布请求。
    Command(RtmpCoreCommand),
}

/// Output action produced by the core for the driver to carry out.
/// 核心产生的输出动作，由 driver 执行。
#[derive(Debug, Clone)]
pub enum CoreOutput {
    /// Bytes that should be sent to the peer.
    /// 应发送给对端的字节。
    Write(Bytes),
    /// A higher-level event for the module to consume.
    /// 供模块消费的高层事件。
    Event(RtmpEvent),
    /// Request a timer to fire at the given absolute time.
    /// 请求在指定绝对时间触发定时器。
    SetTimer { id: TimerId, at_micros: u64 },
    /// Cancel a previously requested timer.
    /// 取消先前请求的定时器。
    CancelTimer { id: TimerId },
}

/// Media type carried by a data message or `MediaData` event.
/// 数据消息或 `MediaData` 事件携带的媒体类型。
#[derive(Debug, Clone)]
pub enum RtmpMediaType {
    /// Audio media data.
    /// 音频媒体数据。
    Audio,
    /// Video media data.
    /// 视频媒体数据。
    Video,
    /// Generic data or metadata.
    /// 通用数据或元数据。
    Data,
}

/// Client-side session state used to track publish/play progress.
/// 客户端会话状态，用于追踪发布/播放进度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpClientState {
    /// Handshake and `connect` have completed.
    /// 握手与 `connect` 已完成。
    Connected,
    /// A media stream has been created via `createStream`.
    /// 已通过 `createStream` 创建媒体流。
    MediaStreamCreated,
    /// The client is publishing media to the server.
    /// 客户端正在向服务端发布媒体。
    Publishing,
    /// The client is playing media from the server.
    /// 客户端正在从服务端播放媒体。
    Playing,
}

/// High-level event emitted by the core for the module to handle.
/// 核心发出供模块处理的高层事件。
#[derive(Debug, Clone)]
pub enum RtmpEvent {
    /// Handshake and `connect` completed; app and tcUrl are known.
    /// 握手与 `connect` 完成；已知 app 与 tcUrl。
    Connected { app: String, tc_url: String },
    /// A peer requested to publish a stream.
    /// 对端请求发布流。
    PublishRequested {
        stream_id: u32,
        app: String,
        tc_url: String,
        stream_name: String,
    },
    /// A peer requested to play a stream.
    /// 对端请求播放流。
    PlayRequested {
        stream_id: u32,
        app: String,
        tc_url: String,
        stream_name: String,
    },
    /// A media stream was created and assigned a stream ID.
    /// 媒体流已创建并分配了流 ID。
    StreamCreated { stream_id: u32 },
    /// A command was received but not handled by this crate.
    /// 收到命令但本 crate 未处理。
    CommandIgnored { name: String, detail: String },
    /// A message was received but not handled by this crate.
    /// 收到消息但本 crate 未处理。
    MessageIgnored { name: String, detail: String },
    /// A user control event was received but not handled.
    /// 收到用户控制事件但本 crate 未处理。
    UserControlIgnored { name: String, detail: String },
    /// An acknowledgement message was received from the peer.
    /// 收到对端的确认消息。
    AckReceived { sequence_number: u32 },
    /// The local acknowledgement window was updated.
    /// 本地确认窗口已更新。
    LocalAckWindowUpdated { size: u32 },
    /// The peer acknowledgement window was updated.
    /// 对端确认窗口已更新。
    PeerAckWindowUpdated { size: u32 },
    /// The client state changed (client mode only).
    /// 客户端状态已改变（仅客户端模式）。
    ClientStateChanged { state: RtmpClientState },
    /// The peer requested to disconnect.
    /// 对端请求断开连接。
    ClientDisconnectRequested { reason: String },
    /// Metadata (@setDataFrame / onMetaData) was received.
    /// 收到元数据（@setDataFrame / onMetaData）。
    Metadata {
        stream_id: u32,
        values: Vec<AmfValue>,
    },
    /// A data/notify message was received.
    /// 收到数据/通知消息。
    Notify {
        stream_id: u32,
        name: String,
        values: Vec<AmfValue>,
    },
    /// Media data was received and should be forwarded to the codec layer.
    /// 收到媒体数据，应转发到编解码层。
    MediaData {
        stream_id: u32,
        timestamp_ms: u32,
        media_type: RtmpMediaType,
        payload: Bytes,
    },
    /// Player requested seek to a position (milliseconds).
    /// 播放器请求定位到指定位置（毫秒）。
    SeekRequested { stream_id: u32, millis: f64 },
    /// Player requested pause or unpause.
    /// 播放器请求暂停或恢复。
    PauseRequested {
        stream_id: u32,
        pause: bool,
        millis: f64,
    },
    /// Player toggled receiveVideo.
    /// 播放器切换了视频接收。
    ReceiveVideo { stream_id: u32, enabled: bool },
    /// Player toggled receiveAudio.
    /// 播放器切换了音频接收。
    ReceiveAudio { stream_id: u32, enabled: bool },
    /// A stream was closed.
    /// 一个流已关闭。
    StreamClosed { stream_id: u32 },
    /// The peer closed the connection.
    /// 对端关闭了连接。
    PeerClosed,
}

/// External command that can be injected into the core to drive outbound behavior.
/// 可注入核心以驱动出站行为的外部命令。
#[derive(Debug, Clone)]
pub enum RtmpCoreCommand {
    /// Update the local acknowledgement window size.
    /// 更新本地确认窗口大小。
    SetWindowAckSize { size: u32 },
    /// Set the peer bandwidth limit.
    /// 设置对端带宽限制。
    SetPeerBandwidth { size: u32 },
    /// Change the outgoing chunk size.
    /// 改变发送 chunk 大小。
    SetChunkSize { size: u32 },
    /// Send an acknowledgement for the given sequence number.
    /// 发送对指定序列号的确认。
    SendAck { sequence_number: u32 },
    /// Send a ping response with the requested timestamp.
    /// 发送带有请求时间戳的 ping 响应。
    SendPingResponse {
        timestamp: crate::timestamp::RtmpTimestamp,
    },
    /// Client: initiate a `connect` command.
    /// 客户端：发起 `connect` 命令。
    ClientConnect {
        app: String,
        flash_ver: String,
        tc_url: String,
    },
    /// Client: create a new media stream.
    /// 客户端：创建新的媒体流。
    ClientCreateStream { transaction_id: f64 },
    /// Client: publish a stream.
    /// 客户端：发布流。
    ClientPublish {
        stream_id: u32,
        transaction_id: f64,
        stream_name: String,
    },
    /// Client: play a stream.
    /// 客户端：播放流。
    ClientPlay {
        stream_id: u32,
        transaction_id: f64,
        stream_name: String,
        start: f64,
    },
    /// Client: request a seek.
    /// 客户端：请求定位。
    ClientSeek { stream_id: u32, millis: f64 },
    /// Client: request pause or resume.
    /// 客户端：请求暂停或恢复。
    ClientPause {
        stream_id: u32,
        pause: bool,
        millis: f64,
    },
    /// Client: a wire command was received and should be parsed.
    /// 客户端：收到线路命令并应解析。
    ClientHandleWireCommand {
        message_stream_id: u32,
        name: String,
        transaction_id: crate::command::TransactionId,
        object: crate::amf::AmfValue,
        args: Vec<crate::amf::AmfValue>,
    },
    /// Client: an acknowledgement was received.
    /// 客户端：收到确认。
    ClientObserveAck { sequence_number: u32 },
    /// Client: window acknowledgement size was received.
    /// 客户端：收到窗口确认大小。
    ClientObserveWinAckSize { size: u32 },
    /// Client: set peer bandwidth message was received.
    /// 客户端：收到设置对端带宽消息。
    ClientHandleSetPeerBandwidth {
        size: u32,
        response_window_size: u32,
    },
    /// Client: media data was observed from the wire.
    /// 客户端：从线路上观察到媒体数据。
    ClientObserveMediaData {
        stream_id: u32,
        timestamp_ms: u32,
        media_type: RtmpMediaType,
        payload: Bytes,
    },
    /// Client: a user control event was received.
    /// 客户端：收到用户控制事件。
    ClientHandleUserControl {
        event: crate::user_control::RtmpUserControlEvent,
    },
    /// Client: an unhandled message was received.
    /// 客户端：收到未处理的消息。
    ClientHandleUnhandledMessage {
        message: crate::message::RtmpMessage,
    },
    /// Server: accept a publish request.
    /// 服务端：接受发布请求。
    AcceptPublish { stream_id: u32 },
    /// Server: reject a publish request.
    /// 服务端：拒绝发布请求。
    RejectPublish { stream_id: u32, description: String },
    /// Server: accept a play request.
    /// 服务端：接受播放请求。
    AcceptPlay { stream_id: u32 },
    /// Server: accept a play request with optional status/sample access.
    /// 服务端：接受播放请求，可选发送状态/sample access。
    AcceptPlayConfigured {
        stream_id: u32,
        emit_play_status: bool,
        emit_sample_access: bool,
    },
    /// Server: reject a play request.
    /// 服务端：拒绝播放请求。
    RejectPlay { stream_id: u32, description: String },
    /// Send metadata to the peer.
    /// 向对端发送元数据。
    SendMetadata {
        stream_id: u32,
        timestamp_ms: u32,
        payload: Bytes,
    },
    /// Send audio data to the peer.
    /// 向对端发送音频数据。
    SendAudio {
        stream_id: u32,
        timestamp_ms: u32,
        payload: Bytes,
    },
    /// Send video data to the peer.
    /// 向对端发送视频数据。
    SendVideo {
        stream_id: u32,
        timestamp_ms: u32,
        payload: Bytes,
    },
    /// Send a data/notify message to the peer.
    /// 向对端发送数据/通知消息。
    SendNotify {
        stream_id: u32,
        timestamp_ms: u32,
        payload: Bytes,
    },
    /// Close the given stream.
    /// 关闭指定流。
    CloseStream { stream_id: u32 },
    /// Close the entire connection.
    /// 关闭整个连接。
    CloseConnection,
}

/// Error type returned by `RtmpCore::handle_input`.
/// `RtmpCore::handle_input` 返回的错误类型。
#[derive(Debug, thiserror::Error)]
pub enum RtmpCoreError {
    /// A chunk or message decoding error occurred.
    /// 发生 chunk 或消息解码错误。
    #[error("chunk: {0}")]
    Chunk(String),
    /// An AMF0 decode/encode failure occurred.
    /// 发生 AMF0 编解码失败。
    #[error("amf0 decode failed: {0}")]
    Amf0(String),
    /// The handshake version byte was not recognized.
    /// 握手版本字节不被识别。
    #[error("invalid rtmp handshake version: {0}")]
    InvalidHandshakeVersion(u8),
    /// A generic handshake failure occurred.
    /// 发生通用握手失败。
    #[error("handshake: {0}")]
    Handshake(String),
}

impl From<RtmpProtocolError> for RtmpCoreError {
    fn from(value: RtmpProtocolError) -> Self {
        Self::Chunk(value.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandshakeState {
    Handshaking,
    WaitC2,
    Ready,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientPendingAction {
    Publish,
    Play,
}

#[derive(Debug)]
struct PendingPublishMedia {
    stream_id: u32,
    timestamp_ms: u32,
    media_type: RtmpMediaType,
    payload: Bytes,
    is_sequence_header: bool,
}

const MAX_PENDING_PUBLISH_MEDIA_EVENTS: usize = 256;
const MAX_PENDING_PUBLISH_MEDIA_BYTES: usize = 8 * 1024 * 1024;

#[allow(clippy::large_enum_variant)]
enum HandshakeRole {
    Server(RtmpServerHandshake),
    Client,
}

impl core::fmt::Debug for HandshakeRole {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Server(h) => f.debug_tuple("Server").field(h).finish(),
            Self::Client => write!(f, "Client"),
        }
    }
}

/// The RTMP protocol core state machine, providing Sans-I/O processing of inputs and outputs.
/// RTMP 协议核心状态机，提供输入与输出的无 I/O 处理。
#[derive(Debug)]
pub struct RtmpCore {
    state: HandshakeState,
    in_chunk_size: usize,
    out_chunk_size: usize,
    decoder: RtmpChunkDecoder,
    encoder: RtmpMessageEncoder,
    input_buf: Buf,
    handshake: HandshakeRole,
    connected_app: Option<String>,
    connected_tc_url: Option<String>,
    peer_ack_window_size: u64,
    local_ack_window_size: u32,
    last_peer_bandwidth_limit_type: crate::message::SetPeerBandwidthLimitType,
    total_bytes_received: u64,
    last_ack_sent: u64,
    active_publish: Option<u32>,
    pending_publish: Option<u32>,
    pending_media: VecDeque<PendingPublishMedia>,
    pending_media_bytes: usize,
    next_stream_id: u32,
    client_create_stream_transaction_id: Option<i64>,
    client_pending_action: Option<ClientPendingAction>,
}

impl Default for RtmpCore {
    fn default() -> Self {
        Self::new()
    }
}

impl RtmpCore {
    /// Creates a new server-side `RtmpCore` starting in the handshake state.
    /// 创建新的服务端 `RtmpCore`，从握手状态开始。
    pub fn new() -> Self {
        let mut encoder = RtmpMessageEncoder::default();
        encoder.set_chunk_size(RtmpChunkSize::saturating_new(60_000));
        Self {
            state: HandshakeState::Handshaking,
            in_chunk_size: 128,
            out_chunk_size: 60_000,
            decoder: RtmpChunkDecoder::default(),
            encoder,
            input_buf: Buf::default(),
            handshake: HandshakeRole::Server(RtmpServerHandshake::new_lenient_seeded_s1()),
            connected_app: None,
            connected_tc_url: None,
            peer_ack_window_size: u64::MAX,
            local_ack_window_size: 5_000_000,
            last_peer_bandwidth_limit_type: crate::message::SetPeerBandwidthLimitType::Soft,
            total_bytes_received: 0,
            last_ack_sent: 0,
            active_publish: None,
            pending_publish: None,
            pending_media: VecDeque::new(),
            pending_media_bytes: 0,
            next_stream_id: 1,
            client_create_stream_transaction_id: None,
            client_pending_action: None,
        }
    }

    /// Creates a new client-side `RtmpCore` that starts in the ready state.
    /// 创建新的客户端 `RtmpCore`，从就绪状态开始。
    pub fn new_client() -> Self {
        let mut encoder = RtmpMessageEncoder::default();
        encoder.set_chunk_size(RtmpChunkSize::saturating_new(60_000));
        Self {
            state: HandshakeState::Ready,
            in_chunk_size: 128,
            out_chunk_size: 60_000,
            decoder: RtmpChunkDecoder::default(),
            encoder,
            input_buf: Buf::default(),
            handshake: HandshakeRole::Client,
            connected_app: None,
            connected_tc_url: None,
            peer_ack_window_size: u64::MAX,
            local_ack_window_size: 5_000_000,
            last_peer_bandwidth_limit_type: crate::message::SetPeerBandwidthLimitType::Soft,
            total_bytes_received: 0,
            last_ack_sent: 0,
            active_publish: None,
            pending_publish: None,
            pending_media: VecDeque::new(),
            pending_media_bytes: 0,
            next_stream_id: 1,
            client_create_stream_transaction_id: None,
            client_pending_action: None,
        }
    }

    /// Drives the state machine with one input and returns all produced outputs.
    /// 用一次输入驱动状态机并返回所有产生的输出。
    pub fn handle_input(&mut self, input: CoreInput) -> Result<Vec<CoreOutput>, RtmpCoreError> {
        let mut out = Vec::new();
        match input {
            CoreInput::Bytes(bytes) => self.on_bytes(bytes, &mut out)?,
            CoreInput::Timeout { .. } => {}
            CoreInput::Command(cmd) => self.on_command(cmd, &mut out)?,
        }
        Ok(out)
    }

    /// Routes incoming bytes to the handshake or ready-state processing logic.
    /// 将收到的字节路由到握手或就绪状态处理逻辑。
    fn on_bytes(&mut self, bytes: Bytes, out: &mut Vec<CoreOutput>) -> Result<(), RtmpCoreError> {
        if self.state == HandshakeState::Closed {
            return Ok(());
        }

        match self.state {
            HandshakeState::Handshaking | HandshakeState::WaitC2 => {
                self.try_handshake(bytes, out)?;
            }
            HandshakeState::Ready => {
                self.process_ready_bytes(bytes, out)?;
            }
            HandshakeState::Closed => {}
        }
        Ok(())
    }

    /// Processes bytes after the handshake, sending acks and decoding chunks.
    /// 在握手之后处理字节，发送确认并解码 chunk。
    fn process_ready_bytes(
        &mut self,
        bytes: Bytes,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        self.total_bytes_received += bytes.len() as u64;
        let unacked_bytes = self.total_bytes_received - self.last_ack_sent;
        if unacked_bytes > self.peer_ack_window_size / 2 {
            self.last_ack_sent = self.total_bytes_received;
            let ack_value = (self.total_bytes_received & 0xFFFF_FFFF) as u32;
            self.send_message(
                2,
                0,
                crate::message::RtmpMessageType::Ack as u8,
                0,
                Bytes::from(ack_value.to_be_bytes().to_vec()),
                out,
            )?;
        }

        self.input_buf.feed(&bytes);
        loop {
            match self.decoder.decode(self.input_buf.get()) {
                Ok((consumed, maybe_chunk)) => {
                    self.input_buf.advance(consumed);
                    if let Some(chunk) = maybe_chunk {
                        self.on_message(chunk, out)?;
                    }
                }
                Err(err) if err.kind == ErrorKind::InsufficientBuffer => break,
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// Dispatches a complete chunk to control, media, command, or aggregate handlers.
    /// 将完整的 chunk 分派到控制、媒体、命令或聚合处理程序。
    fn on_message(
        &mut self,
        msg: crate::chunk::RtmpChunk,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        match msg.message_type {
            RtmpMessageType::SetChunkSize
            | RtmpMessageType::Ack
            | RtmpMessageType::WinAckSize
            | RtmpMessageType::UserControl
            | RtmpMessageType::SetPeerBandwidth
            | RtmpMessageType::Abort => {
                self.handle_control_message(msg, out)?;
            }
            RtmpMessageType::Audio => {
                self.handle_media_input(
                    msg.message_stream_id.get(),
                    msg.timestamp.as_millis(),
                    RtmpMediaType::Audio,
                    msg.payload,
                    out,
                );
            }
            RtmpMessageType::Video => {
                self.handle_media_input(
                    msg.message_stream_id.get(),
                    msg.timestamp.as_millis(),
                    RtmpMediaType::Video,
                    msg.payload,
                    out,
                );
            }
            RtmpMessageType::DataAmf3 => {
                let stream_id = msg.message_stream_id.get();
                let timestamp_ms = msg.timestamp.as_millis();
                let payload = msg.payload;
                match self.on_notify_message_amf3(stream_id, payload.clone()) {
                    Ok(Some(event)) => out.push(CoreOutput::Event(event)),
                    Ok(None) => {
                        self.handle_media_input(
                            stream_id,
                            timestamp_ms,
                            RtmpMediaType::Data,
                            payload,
                            out,
                        );
                    }
                    Err(RtmpCoreError::Amf0(_)) => {}
                    Err(err) => return Err(err),
                }
            }
            RtmpMessageType::DataAmf0 => {
                let stream_id = msg.message_stream_id.get();
                let timestamp_ms = msg.timestamp.as_millis();
                let payload = msg.payload;
                if let Some(event) = self.on_notify_message(stream_id, payload.clone())? {
                    out.push(CoreOutput::Event(event));
                } else {
                    self.handle_media_input(
                        stream_id,
                        timestamp_ms,
                        RtmpMediaType::Data,
                        payload,
                        out,
                    );
                }
            }
            RtmpMessageType::CommandAmf0 => {
                self.on_command_message(msg.message_stream_id.get(), msg.payload, out)?;
            }
            RtmpMessageType::CommandAmf3 => {
                if let Err(err) =
                    self.on_command_message_amf3(msg.message_stream_id.get(), msg.payload, out)
                {
                    if !matches!(err, RtmpCoreError::Amf0(_)) {
                        return Err(err);
                    }
                }
            }
            RtmpMessageType::Aggregate => {
                self.on_aggregate_message(msg, out)?;
            }
        }
        Ok(())
    }

    /// Splits an Aggregate message (type 22) into sub-messages and processes each one.
    /// 将 Aggregate 消息（类型 22）拆分为子消息并逐个处理。
    fn on_aggregate_message(
        &mut self,
        msg: crate::chunk::RtmpChunk,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let base_timestamp = msg.timestamp.as_millis();
        let payload = msg.payload.as_ref();
        let mut pos = 0usize;

        while pos + 11 <= payload.len() {
            let sub_type = payload[pos];
            let sub_length =
                u32::from_be_bytes([0, payload[pos + 1], payload[pos + 2], payload[pos + 3]])
                    as usize;
            let sub_timestamp = u32::from_be_bytes([
                payload[pos + 7], // extended byte
                payload[pos + 4],
                payload[pos + 5],
                payload[pos + 6],
            ]);
            // stream_id at pos+8..pos+11 (ignored, use parent)
            pos += 11;

            if pos + sub_length > payload.len() {
                break; // malformed, stop parsing
            }

            let sub_payload = Bytes::copy_from_slice(&payload[pos..pos + sub_length]);
            pos += sub_length;

            // Skip back pointer (4 bytes)
            if pos + 4 <= payload.len() {
                pos += 4;
            }

            let effective_timestamp = base_timestamp.wrapping_add(sub_timestamp);
            let Ok(sub_msg_type) = RtmpMessageType::from_type_id(sub_type) else {
                continue; // skip unknown sub-message types
            };

            let sub_chunk = crate::chunk::RtmpChunk {
                chunk_stream_id: msg.chunk_stream_id,
                message_stream_id: msg.message_stream_id,
                message_type: sub_msg_type,
                timestamp: crate::timestamp::RtmpTimestamp::from_millis(effective_timestamp),
                payload: sub_payload,
            };
            self.on_message(sub_chunk, out)?;
        }
        Ok(())
    }

    /// Decodes protocol control messages and updates chunk/ack/bandwidth state.
    /// 解码协议控制消息并更新 chunk/确认/带宽状态。
    fn handle_control_message(
        &mut self,
        chunk: crate::chunk::RtmpChunk,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let message = decode_rtmp_chunk_to_message(chunk)?;
        match message {
            crate::message::RtmpMessage::SetChunkSize { size, .. } => {
                self.in_chunk_size = size.get();
                self.decoder.set_chunk_size(size);
            }
            crate::message::RtmpMessage::Ack {
                sequence_number, ..
            } => {
                out.push(CoreOutput::Event(RtmpEvent::AckReceived {
                    sequence_number,
                }));
            }
            crate::message::RtmpMessage::WinAckSize { size, .. } => {
                self.peer_ack_window_size = size as u64;
                out.push(CoreOutput::Event(RtmpEvent::PeerAckWindowUpdated { size }));
            }
            crate::message::RtmpMessage::UserControl {
                event: crate::user_control::RtmpUserControlEvent::PingRequest { timestamp },
                ..
            } => {
                let mut payload = Vec::new();
                crate::user_control::RtmpUserControlEvent::PingResponse { timestamp }
                    .encode(&mut payload);
                self.send_message(
                    2,
                    0,
                    crate::message::RtmpMessageType::UserControl as u8,
                    0,
                    Bytes::from(payload),
                    out,
                )?;
            }
            crate::message::RtmpMessage::SetPeerBandwidth {
                size, limit_type, ..
            } => {
                let effective_type = match limit_type {
                    crate::message::SetPeerBandwidthLimitType::Dynamic => {
                        self.last_peer_bandwidth_limit_type
                    }
                    other => other,
                };
                self.last_peer_bandwidth_limit_type = limit_type;

                let response_size = match effective_type {
                    crate::message::SetPeerBandwidthLimitType::Hard => size,
                    crate::message::SetPeerBandwidthLimitType::Soft
                    | crate::message::SetPeerBandwidthLimitType::Dynamic => size,
                };
                self.local_ack_window_size = response_size;
                out.push(CoreOutput::Event(RtmpEvent::LocalAckWindowUpdated { size }));
                self.send_message(
                    2,
                    0,
                    crate::message::RtmpMessageType::WinAckSize as u8,
                    0,
                    Bytes::from(response_size.to_be_bytes().to_vec()),
                    out,
                )?;
            }
            crate::message::RtmpMessage::Abort {
                chunk_stream_id, ..
            } => {
                self.decoder.reset_chunk_stream(chunk_stream_id);
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
