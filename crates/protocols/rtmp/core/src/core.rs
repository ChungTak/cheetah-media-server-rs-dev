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

/// `TimerId` type alias.
/// `TimerId` 类型别名.
pub type TimerId = u64;

/// `CoreInput` enumeration.
/// `CoreInput` 枚举.
#[derive(Debug, Clone)]
pub enum CoreInput {
    /// `Bytes` variant.
    /// `Bytes` 变体.
    Bytes(Bytes),
    /// `Timeout` variant.
    /// `Timeout` 变体.
    Timeout { id: TimerId },
    /// `Command` variant.
    /// `Command` 变体.
    Command(RtmpCoreCommand),
}

/// `CoreOutput` enumeration.
/// `CoreOutput` 枚举.
#[derive(Debug, Clone)]
pub enum CoreOutput {
    /// `Write` variant.
    /// `Write` 变体.
    Write(Bytes),
    /// `Event` variant.
    /// `Event` 变体.
    Event(RtmpEvent),
    /// `SetTimer` variant.
    /// `SetTimer` 变体.
    SetTimer { id: TimerId, at_micros: u64 },
    /// `CancelTimer` variant.
    /// `CancelTimer` 变体.
    CancelTimer { id: TimerId },
}

/// `RtmpMediaType` enumeration.
/// `RtmpMediaType` 枚举.
#[derive(Debug, Clone)]
pub enum RtmpMediaType {
    /// `Audio` variant.
    /// `Audio` 变体.
    Audio,
    /// `Video` variant.
    /// `Video` 变体.
    Video,
    /// `Data` variant.
    /// `Data` 变体.
    Data,
}

/// `RtmpClientState` enumeration.
/// `RtmpClientState` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpClientState {
    /// `Connected` variant.
    /// `Connected` 变体.
    Connected,
    /// `MediaStreamCreated` variant.
    /// `MediaStreamCreated` 变体.
    MediaStreamCreated,
    /// `Publishing` variant.
    /// `Publishing` 变体.
    Publishing,
    /// `Playing` variant.
    /// `Playing` 变体.
    Playing,
}

/// `RtmpEvent` enumeration.
/// `RtmpEvent` 枚举.
#[derive(Debug, Clone)]
pub enum RtmpEvent {
    /// `Connected` variant.
    /// `Connected` 变体.
    Connected { app: String, tc_url: String },
    /// `PublishRequested` variant.
    /// `PublishRequested` 变体.
    PublishRequested {
        stream_id: u32,
        app: String,
        tc_url: String,
        stream_name: String,
    },
    /// `PlayRequested` variant.
    /// `PlayRequested` 变体.
    PlayRequested {
        stream_id: u32,
        app: String,
        tc_url: String,
        stream_name: String,
    },
    /// `StreamCreated` variant.
    /// `StreamCreated` 变体.
    StreamCreated { stream_id: u32 },
    /// `CommandIgnored` variant.
    /// `CommandIgnored` 变体.
    CommandIgnored { name: String, detail: String },
    /// `MessageIgnored` variant.
    /// `MessageIgnored` 变体.
    MessageIgnored { name: String, detail: String },
    /// `UserControlIgnored` variant.
    /// `UserControlIgnored` 变体.
    UserControlIgnored { name: String, detail: String },
    /// `AckReceived` variant.
    /// `AckReceived` 变体.
    AckReceived { sequence_number: u32 },
    /// `LocalAckWindowUpdated` variant.
    /// `LocalAckWindowUpdated` 变体.
    LocalAckWindowUpdated { size: u32 },
    /// `PeerAckWindowUpdated` variant.
    /// `PeerAckWindowUpdated` 变体.
    PeerAckWindowUpdated { size: u32 },
    /// `ClientStateChanged` variant.
    /// `ClientStateChanged` 变体.
    ClientStateChanged { state: RtmpClientState },
    /// `ClientDisconnectRequested` variant.
    /// `ClientDisconnectRequested` 变体.
    ClientDisconnectRequested { reason: String },
    /// `Metadata` variant.
    /// `Metadata` 变体.
    Metadata {
        stream_id: u32,
        values: Vec<AmfValue>,
    },
    /// `Notify` variant.
    /// `Notify` 变体.
    Notify {
        stream_id: u32,
        name: String,
        values: Vec<AmfValue>,
    },
    /// `MediaData` variant.
    /// `MediaData` 变体.
    MediaData {
        stream_id: u32,
        timestamp_ms: u32,
        media_type: RtmpMediaType,
        payload: Bytes,
    },
    /// Player requested seek to a position (milliseconds).
    SeekRequested { stream_id: u32, millis: f64 },
    /// Player requested pause or unpause.
    PauseRequested {
        stream_id: u32,
        pause: bool,
        millis: f64,
    },
    /// Player toggled receiveVideo.
    ReceiveVideo { stream_id: u32, enabled: bool },
    /// Player toggled receiveAudio.
    ReceiveAudio { stream_id: u32, enabled: bool },
    /// `StreamClosed` variant.
    /// `StreamClosed` 变体.
    StreamClosed { stream_id: u32 },
    /// `PeerClosed` variant.
    /// `PeerClosed` 变体.
    PeerClosed,
}

/// `RtmpCoreCommand` enumeration.
/// `RtmpCoreCommand` 枚举.
#[derive(Debug, Clone)]
pub enum RtmpCoreCommand {
    /// `SetWindowAckSize` variant.
    /// `SetWindowAckSize` 变体.
    SetWindowAckSize { size: u32 },
    /// `SetPeerBandwidth` variant.
    /// `SetPeerBandwidth` 变体.
    SetPeerBandwidth { size: u32 },
    /// `SetChunkSize` variant.
    /// `SetChunkSize` 变体.
    SetChunkSize { size: u32 },
    /// `SendAck` variant.
    /// `SendAck` 变体.
    SendAck { sequence_number: u32 },
    /// `SendPingResponse` variant.
    /// `SendPingResponse` 变体.
    SendPingResponse {
        timestamp: crate::timestamp::RtmpTimestamp,
    },
    /// `ClientConnect` variant.
    /// `ClientConnect` 变体.
    ClientConnect {
        app: String,
        flash_ver: String,
        tc_url: String,
    },
    /// `ClientCreateStream` variant.
    /// `ClientCreateStream` 变体.
    ClientCreateStream { transaction_id: f64 },
    /// `ClientPublish` variant.
    /// `ClientPublish` 变体.
    ClientPublish {
        stream_id: u32,
        transaction_id: f64,
        stream_name: String,
    },
    /// `ClientPlay` variant.
    /// `ClientPlay` 变体.
    ClientPlay {
        stream_id: u32,
        transaction_id: f64,
        stream_name: String,
        start: f64,
    },
    /// `ClientSeek` variant.
    /// `ClientSeek` 变体.
    ClientSeek { stream_id: u32, millis: f64 },
    /// `ClientPause` variant.
    /// `ClientPause` 变体.
    ClientPause {
        stream_id: u32,
        pause: bool,
        millis: f64,
    },
    /// `ClientHandleWireCommand` variant.
    /// `ClientHandleWireCommand` 变体.
    ClientHandleWireCommand {
        message_stream_id: u32,
        name: String,
        transaction_id: crate::command::TransactionId,
        object: crate::amf::AmfValue,
        args: Vec<crate::amf::AmfValue>,
    },
    /// `ClientObserveAck` variant.
    /// `ClientObserveAck` 变体.
    ClientObserveAck { sequence_number: u32 },
    /// `ClientObserveWinAckSize` variant.
    /// `ClientObserveWinAckSize` 变体.
    ClientObserveWinAckSize { size: u32 },
    /// `ClientHandleSetPeerBandwidth` variant.
    /// `ClientHandleSetPeerBandwidth` 变体.
    ClientHandleSetPeerBandwidth {
        size: u32,
        response_window_size: u32,
    },
    /// `ClientObserveMediaData` variant.
    /// `ClientObserveMediaData` 变体.
    ClientObserveMediaData {
        stream_id: u32,
        timestamp_ms: u32,
        media_type: RtmpMediaType,
        payload: Bytes,
    },
    /// `ClientHandleUserControl` variant.
    /// `ClientHandleUserControl` 变体.
    ClientHandleUserControl {
        event: crate::user_control::RtmpUserControlEvent,
    },
    /// `ClientHandleUnhandledMessage` variant.
    /// `ClientHandleUnhandledMessage` 变体.
    ClientHandleUnhandledMessage {
        message: crate::message::RtmpMessage,
    },
    /// `AcceptPublish` variant.
    /// `AcceptPublish` 变体.
    AcceptPublish { stream_id: u32 },
    /// `RejectPublish` variant.
    /// `RejectPublish` 变体.
    RejectPublish { stream_id: u32, description: String },
    /// `AcceptPlay` variant.
    /// `AcceptPlay` 变体.
    AcceptPlay { stream_id: u32 },
    /// `AcceptPlayConfigured` variant.
    /// `AcceptPlayConfigured` 变体.
    AcceptPlayConfigured {
        stream_id: u32,
        emit_play_status: bool,
        emit_sample_access: bool,
    },
    /// `RejectPlay` variant.
    /// `RejectPlay` 变体.
    RejectPlay { stream_id: u32, description: String },
    /// `SendMetadata` variant.
    /// `SendMetadata` 变体.
    SendMetadata {
        stream_id: u32,
        timestamp_ms: u32,
        payload: Bytes,
    },
    /// `SendAudio` variant.
    /// `SendAudio` 变体.
    SendAudio {
        stream_id: u32,
        timestamp_ms: u32,
        payload: Bytes,
    },
    /// `SendVideo` variant.
    /// `SendVideo` 变体.
    SendVideo {
        stream_id: u32,
        timestamp_ms: u32,
        payload: Bytes,
    },
    /// `SendNotify` variant.
    /// `SendNotify` 变体.
    SendNotify {
        stream_id: u32,
        timestamp_ms: u32,
        payload: Bytes,
    },
    /// `CloseStream` variant.
    /// `CloseStream` 变体.
    CloseStream { stream_id: u32 },
    /// `CloseConnection` variant.
    /// `CloseConnection` 变体.
    CloseConnection,
}

/// `RtmpCoreError` enumeration.
/// `RtmpCoreError` 枚举.
#[derive(Debug, thiserror::Error)]
pub enum RtmpCoreError {
    /// `Chunk` variant.
    /// `Chunk` 变体.
    #[error("chunk: {0}")]
    Chunk(String),
    /// `Amf0` variant.
    /// `Amf0` 变体.
    #[error("amf0 decode failed: {0}")]
    Amf0(String),
    /// `InvalidHandshakeVersion` variant.
    /// `InvalidHandshakeVersion` 变体.
    #[error("invalid rtmp handshake version: {0}")]
    InvalidHandshakeVersion(u8),
    /// `Handshake` variant.
    /// `Handshake` 变体.
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

/// `RtmpCore` data structure.
/// `RtmpCore` 数据结构.
#[derive(Debug)]
pub struct RtmpCore {
    /// `state` field of type `HandshakeState`.
    /// `state` 字段，类型为 `HandshakeState`.
    state: HandshakeState,
    /// `in_chunk_size` field of type `usize`.
    /// `in_chunk_size` 字段，类型为 `usize`.
    in_chunk_size: usize,
    /// `out_chunk_size` field of type `usize`.
    /// `out_chunk_size` 字段，类型为 `usize`.
    out_chunk_size: usize,
    /// `decoder` field of type `RtmpChunkDecoder`.
    /// `decoder` 字段，类型为 `RtmpChunkDecoder`.
    decoder: RtmpChunkDecoder,
    /// `encoder` field of type `RtmpMessageEncoder`.
    /// `encoder` 字段，类型为 `RtmpMessageEncoder`.
    encoder: RtmpMessageEncoder,
    /// `input_buf` field of type `Buf`.
    /// `input_buf` 字段，类型为 `Buf`.
    input_buf: Buf,
    /// `handshake` field of type `HandshakeRole`.
    /// `handshake` 字段，类型为 `HandshakeRole`.
    handshake: HandshakeRole,
    /// `connected_app` field.
    /// `connected_app` 字段.
    connected_app: Option<String>,
    /// `connected_tc_url` field.
    /// `connected_tc_url` 字段.
    connected_tc_url: Option<String>,
    /// `peer_ack_window_size` field of type `u64`.
    /// `peer_ack_window_size` 字段，类型为 `u64`.
    peer_ack_window_size: u64,
    /// `local_ack_window_size` field of type `u32`.
    /// `local_ack_window_size` 字段，类型为 `u32`.
    local_ack_window_size: u32,
    /// `last_peer_bandwidth_limit_type` field.
    /// `last_peer_bandwidth_limit_type` 字段.
    last_peer_bandwidth_limit_type: crate::message::SetPeerBandwidthLimitType,
    /// `total_bytes_received` field of type `u64`.
    /// `total_bytes_received` 字段，类型为 `u64`.
    total_bytes_received: u64,
    /// `last_ack_sent` field of type `u64`.
    /// `last_ack_sent` 字段，类型为 `u64`.
    last_ack_sent: u64,
    /// `active_publish` field.
    /// `active_publish` 字段.
    active_publish: Option<u32>,
    /// `pending_publish` field.
    /// `pending_publish` 字段.
    pending_publish: Option<u32>,
    /// `pending_media` field.
    /// `pending_media` 字段.
    pending_media: VecDeque<PendingPublishMedia>,
    /// `pending_media_bytes` field of type `usize`.
    /// `pending_media_bytes` 字段，类型为 `usize`.
    pending_media_bytes: usize,
    /// `next_stream_id` field of type `u32`.
    /// `next_stream_id` 字段，类型为 `u32`.
    next_stream_id: u32,
    /// `client_create_stream_transaction_id` field.
    /// `client_create_stream_transaction_id` 字段.
    client_create_stream_transaction_id: Option<i64>,
    /// `client_pending_action` field.
    /// `client_pending_action` 字段.
    client_pending_action: Option<ClientPendingAction>,
}

impl Default for RtmpCore {
    fn default() -> Self {
        Self::new()
    }
}

impl RtmpCore {
    /// Creates a new instance.
    /// 创建 新的 实例.
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

    /// Creates a new `client` instance.
    /// 创建 新的 `client` 实例.
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

    /// `handle_input` function.
    /// `handle_input` 函数.
    pub fn handle_input(&mut self, input: CoreInput) -> Result<Vec<CoreOutput>, RtmpCoreError> {
        let mut out = Vec::new();
        match input {
            CoreInput::Bytes(bytes) => self.on_bytes(bytes, &mut out)?,
            CoreInput::Timeout { .. } => {}
            CoreInput::Command(cmd) => self.on_command(cmd, &mut out)?,
        }
        Ok(out)
    }

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

    /// Split an Aggregate message (type 22) into sub-messages and process each.
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
