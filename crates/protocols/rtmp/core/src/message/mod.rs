/// Message decoding logic: turns complete chunks into typed RTMP messages.
/// 消息解码逻辑：将完整的 chunk 转换为类型化的 RTMP 消息。
pub mod decoder;
/// Message encoding logic: turns typed RTMP messages into chunk payloads.
/// 消息编码逻辑：将类型化的 RTMP 消息编码为 chunk 负载。
pub mod encoder;

pub use decoder::{decode_rtmp_chunk_to_message, RtmpMessageDecoder};
pub use encoder::RtmpMessageEncoder;

use crate::amf::{AmfValue, AmfVersion};
use crate::chunk::{RtmpChunkSize, RtmpChunkStreamId};
use crate::command::TransactionId;
use crate::error::Error;
use crate::media::{AudioFrame, MediaFrame, VideoFrame};
use crate::prelude::*;
use crate::timestamp::RtmpTimestamp;
use crate::user_control::RtmpUserControlEvent;

use bytes::Bytes;

/// RTMP message stream identifier (MSID) used to multiplex messages on a single connection.
/// RTMP 消息流标识符（MSID），用于在单个连接上复用消息。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RtmpMessageStreamId(u32);

impl RtmpMessageStreamId {
    /// Protocol control stream ID, fixed to 0 by the RTMP spec.
    /// 控制流的 ID 按规范固定为 0。
    pub const PCM: Self = Self(0);

    /// First server-assigned stream ID; its purpose is not used but kept as 1.
    /// 服务器首次分配的流 ID，实际未使用，但固定为 1。
    pub const FIRST: Self = Self(1);

    /// Media stream ID used when this crate only handles one stream per connection.
    /// 媒体流 ID，在本 crate 中一个连接仅处理一个流，因此使用固定值 2。
    pub const MEDIA: Self = Self(2);

    /// Wraps a raw message stream ID.
    /// 包装原始消息流 ID。
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// Returns the raw message stream ID value.
    /// 返回原始消息流 ID 值。
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// RTMP message type discriminator for protocol, media and command messages.
/// RTMP 消息类型区分符，涵盖协议、媒体与命令消息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpMessageType {
    // Protocol Control Messages
    /// Notify the peer to change the chunk size used on the wire.
    /// 通知对端改变在线路上使用的 chunk 大小。
    SetChunkSize = 1,
    /// Instructs the peer to discard chunks for a given chunk stream.
    /// 指示对端丢弃指定 chunk 流上的 chunk。
    Abort = 2,
    /// Acknowledges the number of bytes received so far.
    /// 确认到目前为止已接收的字节数。
    Ack = 3,
    /// User control event such as StreamBegin or Ping.
    /// 用户控制事件，如 StreamBegin 或 Ping。
    UserControl = 4,
    /// Sets the peer's window acknowledgement size.
    /// 设置对端的窗口确认大小。
    WinAckSize = 5,
    /// Limits the peer's outgoing bandwidth.
    /// 限制对端的发送带宽。
    SetPeerBandwidth = 6,

    // Media Messages
    /// Audio frame data.
    /// 音频帧数据。
    Audio = 8,
    /// Video frame data.
    /// 视频帧数据。
    Video = 9,

    // Data/Command Messages
    /// Metadata/data message encoded with AMF3.
    /// 使用 AMF3 编码的元数据/数据消息。
    DataAmf3 = 15,
    /// Command message encoded with AMF3.
    /// 使用 AMF3 编码的命令消息。
    CommandAmf3 = 17,
    /// Metadata/data message encoded with AMF0.
    /// 使用 AMF0 编码的元数据/数据消息。
    DataAmf0 = 18,
    /// Command message encoded with AMF0.
    /// 使用 AMF0 编码的命令消息。
    CommandAmf0 = 20,
    // Aggregate Message
    /// Aggregate message containing multiple smaller RTMP messages.
    /// 聚合消息，包含多个小型 RTMP 消息。
    Aggregate = 22,
}

impl RtmpMessageType {
    /// Maps a raw message type ID to the typed enum, rejecting unknown values.
    /// 将原始消息类型 ID 映射为类型化枚举，未知值会被拒绝。
    pub fn from_type_id(type_id: u8) -> Result<Self, Error> {
        match type_id {
            1 => Ok(RtmpMessageType::SetChunkSize),
            2 => Ok(RtmpMessageType::Abort),
            3 => Ok(RtmpMessageType::Ack),
            4 => Ok(RtmpMessageType::UserControl),
            5 => Ok(RtmpMessageType::WinAckSize),
            6 => Ok(RtmpMessageType::SetPeerBandwidth),
            8 => Ok(RtmpMessageType::Audio),
            9 => Ok(RtmpMessageType::Video),
            15 => Ok(RtmpMessageType::DataAmf3),
            17 => Ok(RtmpMessageType::CommandAmf3),
            18 => Ok(RtmpMessageType::DataAmf0),
            20 => Ok(RtmpMessageType::CommandAmf0),
            22 => Ok(RtmpMessageType::Aggregate),
            _ => Err(Error::invalid_data(format!(
                "unknown message type: {type_id}"
            ))),
        }
    }
}

/// Message header shared by every RTMP message.
/// 所有 RTMP 消息共享的消息头部。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtmpMessageHeader {
    pub stream_id: RtmpMessageStreamId,
    pub timestamp: RtmpTimestamp,
}

impl RtmpMessageHeader {
    /// Predefined header for the protocol control message stream.
    /// 协议控制消息流的预定义头部。
    pub const PCM: Self = Self {
        stream_id: RtmpMessageStreamId::PCM,
        timestamp: RtmpTimestamp::ZERO,
    };
}

/// A typed RTMP message carrying either a protocol control, media, command or data payload.
/// 类型化的 RTMP 消息，携带协议控制、媒体、命令或数据负载。
#[derive(Debug, Clone, PartialEq)]
pub enum RtmpMessage {
    /// Request the peer to change the chunk size used on the wire.
    /// 请求对端改变在线路上使用的 chunk 大小。
    SetChunkSize {
        header: RtmpMessageHeader,
        size: RtmpChunkSize,
    },
    /// Instructs the peer to discard chunks on the specified chunk stream.
    /// 指示对端丢弃指定 chunk 流上的 chunk。
    Abort {
        header: RtmpMessageHeader,
        chunk_stream_id: RtmpChunkStreamId,
    },
    /// Acknowledges the total number of bytes received from the peer.
    /// 确认已从对端接收的总字节数。
    Ack {
        header: RtmpMessageHeader,
        sequence_number: u32, // 规范中的名称虽然是序列号，但实际上是累计接收字节数
    },
    /// Sets the peer's window acknowledgement size.
    /// 设置对端的窗口确认大小。
    WinAckSize {
        header: RtmpMessageHeader,
        size: u32,
    },
    /// Sets the peer's outgoing bandwidth limit.
    /// 设置对端的发送带宽限制。
    SetPeerBandwidth {
        header: RtmpMessageHeader,
        size: u32,
        limit_type: SetPeerBandwidthLimitType,
    },
    /// User control event such as StreamBegin, Ping, or BufferReady.
    /// 用户控制事件，如 StreamBegin、Ping 或 BufferReady。
    UserControl {
        header: RtmpMessageHeader,
        event: RtmpUserControlEvent,
    },
    /// Audio frame with parsed metadata and raw payload.
    /// 音频帧，包含已解析的元数据与原始负载。
    Audio {
        header: RtmpMessageHeader,
        frame: AudioFrame,
        payload: Bytes,
    },
    /// Video frame with parsed metadata and raw payload.
    /// 视频帧，包含已解析的元数据与原始负载。
    Video {
        header: RtmpMessageHeader,
        frame: VideoFrame,
        payload: Bytes,
    },
    /// RTMP command such as connect, publish, play, or _result.
    /// RTMP 命令，如 connect、publish、play 或 _result。
    Command {
        header: RtmpMessageHeader,
        amf_version: AmfVersion,
        name: String,
        transaction_id: TransactionId,
        object: AmfValue,
        args: Vec<AmfValue>,
    },
    /// Data/notify message such as @setDataFrame or metadata.
    /// 数据/通知消息，如 @setDataFrame 或 metadata。
    Data {
        header: RtmpMessageHeader,
        amf_version: AmfVersion,
        values: Vec<AmfValue>,
    },
}

impl RtmpMessage {
    /// Returns the shared header for this message.
    /// 返回该消息共享的头部。
    pub fn header(&self) -> RtmpMessageHeader {
        match self {
            RtmpMessage::SetChunkSize { header, .. }
            | RtmpMessage::Abort { header, .. }
            | RtmpMessage::Ack { header, .. }
            | RtmpMessage::WinAckSize { header, .. }
            | RtmpMessage::SetPeerBandwidth { header, .. }
            | RtmpMessage::UserControl { header, .. }
            | RtmpMessage::Audio { header, .. }
            | RtmpMessage::Video { header, .. }
            | RtmpMessage::Command { header, .. }
            | RtmpMessage::Data { header, .. } => *header,
        }
    }

    /// Extracts the media frame from an Audio or Video message; panics otherwise.
    /// 从 Audio 或 Video 消息中提取媒体帧，否则 panic。
    pub fn frame(&self) -> MediaFrame {
        match self {
            RtmpMessage::Audio { frame, .. } => MediaFrame::Audio(frame.clone()),
            RtmpMessage::Video { frame, .. } => MediaFrame::Video(frame.clone()),
            _ => unreachable!("frame() called on non-media message"),
        }
    }

    /// Maps the message variant to its wire type ID, including the AMF version for commands/data.
    /// 将消息变体映射到线路类型 ID，对命令/数据还会区分 AMF 版本。
    pub fn message_type(&self) -> RtmpMessageType {
        match self {
            RtmpMessage::SetChunkSize { .. } => RtmpMessageType::SetChunkSize,
            RtmpMessage::Abort { .. } => RtmpMessageType::Abort,
            RtmpMessage::Ack { .. } => RtmpMessageType::Ack,
            RtmpMessage::WinAckSize { .. } => RtmpMessageType::WinAckSize,
            RtmpMessage::SetPeerBandwidth { .. } => RtmpMessageType::SetPeerBandwidth,
            RtmpMessage::UserControl { .. } => RtmpMessageType::UserControl,
            RtmpMessage::Audio { .. } => RtmpMessageType::Audio,
            RtmpMessage::Video { .. } => RtmpMessageType::Video,
            RtmpMessage::Command { amf_version, .. } => match amf_version {
                AmfVersion::Amf0 => RtmpMessageType::CommandAmf0,
                AmfVersion::Amf3 => RtmpMessageType::CommandAmf3,
            },
            RtmpMessage::Data { amf_version, .. } => match amf_version {
                AmfVersion::Amf0 => RtmpMessageType::DataAmf0,
                AmfVersion::Amf3 => RtmpMessageType::DataAmf3,
            },
        }
    }

    /// Builds a `StreamBegin` user control message for the given stream ID.
    /// 为指定流 ID 构建 `StreamBegin` 用户控制消息。
    pub fn stream_begin(stream_id: RtmpMessageStreamId) -> Self {
        Self::UserControl {
            header: RtmpMessageHeader::PCM,
            event: RtmpUserControlEvent::StreamBegin { stream_id },
        }
    }

    /// Builds a `WinAckSize` protocol control message.
    /// 构建 `WinAckSize` 协议控制消息。
    pub fn win_ack_size(size: u32) -> Self {
        Self::WinAckSize {
            header: RtmpMessageHeader::PCM,
            size,
        }
    }

    /// Builds a `SetPeerBandwidth` message using the hard limit behavior.
    /// 使用 Hard 限制行为构建 `SetPeerBandwidth` 消息。
    pub fn set_peer_bandwidth(size: u32) -> Self {
        Self::SetPeerBandwidth {
            header: RtmpMessageHeader::PCM,
            size,

            // 暂时固定使用行为最简单的 Hard 模式
            limit_type: SetPeerBandwidthLimitType::Hard,
        }
    }

    /// Builds an `Ack` message carrying the total bytes received so far.
    /// 构建携带已接收总字节数的 `Ack` 消息。
    pub fn ack(total_bytes_received: u32) -> Self {
        Self::Ack {
            header: RtmpMessageHeader::PCM,
            sequence_number: total_bytes_received,
        }
    }

    /// Builds a `SetChunkSize` message with the requested chunk size.
    /// 构建带有请求 chunk 大小的 `SetChunkSize` 消息。
    pub fn set_chunk_size(size: RtmpChunkSize) -> Self {
        Self::SetChunkSize {
            header: RtmpMessageHeader::PCM,
            size,
        }
    }
}

/// Bandwidth limit behavior for `SetPeerBandwidth`.
/// `SetPeerBandwidth` 的带宽限制行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetPeerBandwidthLimitType {
    /// The peer must immediately switch to the given limit.
    /// 对端必须立即切换到给定限制。
    Hard = 0,
    /// The peer should use the minimum of the local and given limits.
    /// 对端应取本地限制与给定限制的最小值。
    Soft,
    /// The peer may choose between the previous Hard/Soft behavior.
    /// 对端可在之前的 Hard/Soft 行为之间切换。
    Dynamic,
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::amf::Pair;
    use crate::amf0::Amf0Value;
    use crate::amf3::Amf3Value;
    use crate::media::{AudioFormat, AudioSampleRate};
    use crate::message::RtmpMessageDecoder;
    use crate::message::RtmpMessageEncoder;
    use crate::timestamp::RtmpTimestampDelta;

    fn header(stream_id: u32, timestamp_ms: u32) -> RtmpMessageHeader {
        RtmpMessageHeader {
            stream_id: RtmpMessageStreamId::new(stream_id),
            timestamp: RtmpTimestamp::from_millis(timestamp_ms),
        }
    }

    fn pcm_header(timestamp_ms: u32) -> RtmpMessageHeader {
        RtmpMessageHeader {
            stream_id: RtmpMessageStreamId::PCM,
            timestamp: RtmpTimestamp::from_millis(timestamp_ms),
        }
    }

    fn encode_decode_roundtrip(message: RtmpMessage) -> RtmpMessage {
        let chunk_stream_id = RtmpChunkStreamId::new(3).unwrap();
        let mut encoder = RtmpMessageEncoder::default();
        let mut buf = Vec::new();

        encoder.encode(&mut buf, chunk_stream_id, message);

        let mut decoder = RtmpMessageDecoder::default();

        decoder.feed_buf(&buf);
        decoder.decode().unwrap().unwrap()
    }

    #[test]
    fn test_set_chunk_size_decode_encode() {
        let msg = RtmpMessage::SetChunkSize {
            header: pcm_header(0),
            size: RtmpChunkSize::new(1234).unwrap(),
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_abort_decode_encode() {
        let msg = RtmpMessage::Abort {
            header: pcm_header(0),
            chunk_stream_id: RtmpChunkStreamId::new(10).unwrap(),
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_ack_decode_encode() {
        let msg = RtmpMessage::Ack {
            header: pcm_header(0),
            sequence_number: 56789,
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_win_ack_size_decode_encode() {
        let msg = RtmpMessage::WinAckSize {
            header: pcm_header(0),
            size: 45678,
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_set_peer_bandwidth_decode_encode() {
        let msg = RtmpMessage::SetPeerBandwidth {
            header: pcm_header(0),
            size: 4567,
            limit_type: SetPeerBandwidthLimitType::Soft,
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_user_control_stream_begin_decode_encode() {
        let msg = RtmpMessage::UserControl {
            header: pcm_header(0),
            event: RtmpUserControlEvent::StreamBegin {
                stream_id: RtmpMessageStreamId::new(10),
            },
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_user_control_stream_eof_decode_encode() {
        let msg = RtmpMessage::UserControl {
            header: pcm_header(0),
            event: RtmpUserControlEvent::StreamEof {
                stream_id: RtmpMessageStreamId::new(10),
            },
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_user_control_stream_dry_decode_encode() {
        let msg = RtmpMessage::UserControl {
            header: pcm_header(0),
            event: RtmpUserControlEvent::StreamDry {
                stream_id: RtmpMessageStreamId::new(10),
            },
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_user_control_set_buffer_length_decode_encode() {
        let msg = RtmpMessage::UserControl {
            header: pcm_header(0),
            event: RtmpUserControlEvent::SetBufferLength {
                stream_id: RtmpMessageStreamId::new(10),
                length: 1234,
            },
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_user_control_stream_is_recorded_decode_encode() {
        let msg = RtmpMessage::UserControl {
            header: pcm_header(0),
            event: RtmpUserControlEvent::StreamIsRecorded {
                stream_id: RtmpMessageStreamId::new(10),
            },
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_user_control_ping_request_decode_encode() {
        let msg = RtmpMessage::UserControl {
            header: pcm_header(0),
            event: RtmpUserControlEvent::PingRequest {
                timestamp: RtmpTimestamp::from_millis(3456),
            },
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_user_control_ping_response_decode_encode() {
        let msg = RtmpMessage::UserControl {
            header: pcm_header(0),
            event: RtmpUserControlEvent::PingResponse {
                timestamp: RtmpTimestamp::from_millis(3456),
            },
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_user_control_buffer_empty_decode_encode() {
        let msg = RtmpMessage::UserControl {
            header: pcm_header(0),
            event: RtmpUserControlEvent::BufferEmpty {
                stream_id: RtmpMessageStreamId::new(10),
            },
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_user_control_buffer_ready_decode_encode() {
        let msg = RtmpMessage::UserControl {
            header: pcm_header(0),
            event: RtmpUserControlEvent::BufferReady {
                stream_id: RtmpMessageStreamId::new(10),
            },
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_audio_decode_encode() {
        let frame = AudioFrame {
            timestamp: RtmpTimestamp::from_millis(1234),
            format: AudioFormat::Aac,
            sample_rate: AudioSampleRate::Khz44,
            is_8bit_sample: true,
            is_stereo: true,
            is_aac_sequence_header: false,
            data: b"dummy_audio_data".to_vec(),
        };
        let msg = RtmpMessage::Audio {
            header: header(10, 1234),
            frame: frame.clone(),
            payload: Bytes::new(),
        };

        let decoded = encode_decode_roundtrip(msg);
        assert_eq!(decoded.header(), header(10, 1234));
        assert_eq!(decoded.frame().unwrap_audio(), frame);
    }

    #[test]
    fn test_video_decode_encode() {
        let frame = VideoFrame {
            timestamp: RtmpTimestamp::from_millis(1234),
            composition_timestamp_offset: RtmpTimestampDelta::from_millis(1234),
            frame_type: crate::media::VideoFrameType::KeyFrame,
            codec: crate::media::VideoCodec::Avc,
            avc_packet_type: Some(crate::media::AvcPacketType::NalUnit),
            data: b"dummy_video_data".to_vec(),
        };
        let msg = RtmpMessage::Video {
            header: header(10, 1234),
            frame: frame.clone(),
            payload: Bytes::new(),
        };

        let decoded = encode_decode_roundtrip(msg);
        assert_eq!(decoded.header(), header(10, 1234));
        assert_eq!(decoded.frame().unwrap_video(), frame);
    }

    #[test]
    fn test_command_amf0_decode_encode() {
        let msg = RtmpMessage::Command {
            header: header(10, 0),
            amf_version: AmfVersion::Amf0,
            name: "connect".to_string(),
            transaction_id: TransactionId::CONNECT,
            object: AmfValue::Amf0(Amf0Value::Object {
                class_name: None,
                entries: vec![Pair {
                    key: "a".to_string(),
                    value: Amf0Value::String("b".to_string()),
                }],
            }),
            args: vec![
                AmfValue::Amf0(Amf0Value::String("string".to_string())),
                AmfValue::Amf0(Amf0Value::Array {
                    entries: vec![
                        Amf0Value::Number(1.0),
                        Amf0Value::Number(2.0),
                        Amf0Value::Number(3.0),
                    ],
                }),
            ],
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_command_amf3_decode_encode() {
        let msg = RtmpMessage::Command {
            header: header(10, 0),
            amf_version: AmfVersion::Amf3,
            name: "connect".to_string(),
            transaction_id: TransactionId::CONNECT,
            object: AmfValue::Amf3(Amf3Value::Object {
                class_name: None,
                sealed_count: 1,
                entries: vec![Pair {
                    key: "a".to_string(),
                    value: Amf3Value::String("b".to_string()),
                }],
            }),
            args: vec![
                AmfValue::Amf3(Amf3Value::String("string".to_string())),
                AmfValue::Amf3(Amf3Value::Array {
                    assoc_entries: vec![],
                    dense_entries: vec![
                        Amf3Value::Double(1.0),
                        Amf3Value::Double(2.0),
                        Amf3Value::Double(3.0),
                    ],
                }),
            ],
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_data_amf0_decode_encode() {
        let msg = RtmpMessage::Data {
            header: header(10, 0),
            amf_version: AmfVersion::Amf0,
            values: vec![AmfValue::Amf0(Amf0Value::Object {
                class_name: None,
                entries: vec![
                    Pair {
                        key: "a".to_string(),
                        value: Amf0Value::String("b".to_string()),
                    },
                    Pair {
                        key: "c".to_string(),
                        value: Amf0Value::Number(10.4),
                    },
                ],
            })],
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_data_amf3_decode_encode() {
        let msg = RtmpMessage::Data {
            header: header(10, 0),
            amf_version: AmfVersion::Amf3,
            values: vec![AmfValue::Amf3(Amf3Value::Object {
                class_name: None,
                sealed_count: 2,
                entries: vec![
                    Pair {
                        key: "a".to_string(),
                        value: Amf3Value::String("b".to_string()),
                    },
                    Pair {
                        key: "c".to_string(),
                        value: Amf3Value::Double(10.4),
                    },
                ],
            })],
        };

        let decoded = encode_decode_roundtrip(msg.clone());
        assert_eq!(msg, decoded);
    }
}
