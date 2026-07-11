//! FLV ingress mapping: FLV tags -> canonical `AVFrame` and `TrackInfo`.
//!
//! This is the shared FLV demux ingestion layer consumed by HTTP-FLV and RTMP
//! pull paths. It keeps protocol-specific framing in `cheetah-codec` so
//! downstream modules do not duplicate codec, timestamp, NALU or parameter-set
//! logic.
//!
//! FLV 入口映射：FLV tag -> 标准 `AVFrame` 与 `TrackInfo`。
//!
//! 这是 HTTP-FLV 与 RTMP 拉流路径共享的 FLV 解复用入口层，将协议相关
//! 成帧保留在 `cheetah-codec` 中，避免下游模块重复 codec、时间戳、NALU
//! 或参数集逻辑。

use bytes::Bytes;

use crate::audio::{aac_channel_count_from_config, AacAudioSpecificConfig};
use crate::flv::{parse_avcc_parameter_sets, FlvDemuxEvent, FlvTag, FlvTagType};
use crate::frame::{AVFrame, FrameFlags, FrameFormat, FrameOrigin, RtmpTimestamp, SourceTimestamp};
use crate::frame_view::annexb_from_payload;
use crate::time::Timebase;
use crate::track::{CodecExtradata, CodecId, MediaKind, TrackId, TrackInfo, TrackReadiness};

/// An output produced by [`FlvIngress`] while processing a demuxed FLV event.
///
/// [`FlvIngress`] 处理 FLV 解复用事件时产生的输出。
#[derive(Debug, Clone)]
pub enum FlvIngressOutput {
    /// The track list has changed (new codec config, new track, etc.).
    ///
    /// 轨道列表已改变（新编解码器配置、新轨道等）。
    Track(Vec<TrackInfo>),
    /// A media frame ready to be dispatched.
    ///
    /// 可分发媒体帧。
    Frame(Box<AVFrame>),
}

/// Errors that can occur while mapping FLV tags to canonical frames.
///
/// FLV tag 映射到标准帧时可能发生的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum FlvIngressError {
    #[error("payload too short for tag type {tag_type:?}")]
    PayloadTooShort { tag_type: FlvTagType },
    #[error("unsupported FLV codec {codec_id} in {tag_type:?}")]
    UnsupportedCodec { tag_type: FlvTagType, codec_id: u8 },
    #[error("invalid {tag_type:?} packet type {packet_type}")]
    InvalidPacketType {
        tag_type: FlvTagType,
        packet_type: u8,
    },
    #[error("failed to parse {detail}")]
    ParseFailed { detail: &'static str },
    #[error("no track configured for {tag_type:?}")]
    TrackMissing { tag_type: FlvTagType },
}

/// State that keeps track of FLV tracks and converts incoming tags to
/// canonical `AVFrame` values.
///
/// 维护 FLV 轨道状态并将传入 tag 转换为标准 `AVFrame` 的状态机。
#[derive(Debug, Clone, Default)]
pub struct FlvIngress {
    tracks: Vec<TrackInfo>,
    last_raw_timestamp: u64,
    epoch_offset: u64,
    next_track_id: u32,
    has_video: bool,
    has_audio: bool,
    video_track_id: Option<TrackId>,
    audio_track_id: Option<TrackId>,
}

impl FlvIngress {
    /// Create a new ingress mapper.
    ///
    /// 创建新的入口映射器。
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the currently configured tracks.
    ///
    /// 返回当前已配置的轨道。
    pub fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }

    /// Process one FLV demux event and emit any output.
    ///
    /// 处理一个 FLV 解复用事件并发出输出。
    pub fn process_event(
        &mut self,
        event: FlvDemuxEvent,
    ) -> Result<Option<FlvIngressOutput>, FlvIngressError> {
        match event {
            FlvDemuxEvent::Header(header) => {
                self.has_video = header.has_video;
                self.has_audio = header.has_audio;
                Ok(None)
            }
            FlvDemuxEvent::Tag(tag) => self.process_tag(tag),
            FlvDemuxEvent::PreviousTagSizeMismatch(_) => Ok(None),
        }
    }

    fn process_tag(&mut self, tag: FlvTag) -> Result<Option<FlvIngressOutput>, FlvIngressError> {
        match tag.tag_type {
            FlvTagType::Video => self.process_video_tag(tag),
            FlvTagType::Audio => self.process_audio_tag(tag),
            FlvTagType::Script => {
                // Script tags are ignored for now; onMetaData width/height could be parsed later.
                Ok(None)
            }
        }
    }

    fn process_video_tag(
        &mut self,
        tag: FlvTag,
    ) -> Result<Option<FlvIngressOutput>, FlvIngressError> {
        if tag.payload.is_empty() {
            return Err(FlvIngressError::PayloadTooShort {
                tag_type: FlvTagType::Video,
            });
        }

        let byte0 = tag.payload[0];
        let frame_type = (byte0 >> 4) & 0x0f;
        let codec_id = byte0 & 0x0f;

        // Legacy H.264
        if codec_id == 7 {
            return self.process_h264_video_tag(&tag, frame_type);
        }

        // H.265 legacy codec id
        if codec_id == 12 {
            return self.process_h265_video_tag(&tag, frame_type);
        }

        Err(FlvIngressError::UnsupportedCodec {
            tag_type: FlvTagType::Video,
            codec_id,
        })
    }

    fn process_h264_video_tag(
        &mut self,
        tag: &FlvTag,
        frame_type: u8,
    ) -> Result<Option<FlvIngressOutput>, FlvIngressError> {
        if tag.payload.len() < 5 {
            return Err(FlvIngressError::PayloadTooShort {
                tag_type: FlvTagType::Video,
            });
        }

        let packet_type = tag.payload[1];
        let cts = sign_extend_i24(u32::from_be_bytes([
            0,
            tag.payload[2],
            tag.payload[3],
            tag.payload[4],
        ]));

        match packet_type {
            0 => {
                // AVC sequence header -> AVCDecoderConfigurationRecord
                let avcc = &tag.payload[5..];
                if avcc.is_empty() {
                    return Err(FlvIngressError::ParseFailed {
                        detail: "empty AVC configuration record",
                    });
                }
                let (sps, pps) = parse_avcc_parameter_sets(avcc);
                if sps.is_empty() || pps.is_empty() {
                    return Err(FlvIngressError::ParseFailed {
                        detail: "missing SPS or PPS in AVC configuration record",
                    });
                }

                let track_id = self.video_track_id();
                self.add_or_update_track(TrackInfo {
                    track_id,
                    media_kind: MediaKind::Video,
                    codec: CodecId::H264,
                    aac_rtp_packetization: Default::default(),
                    aac_latm_config_in_band: false,
                    payload_type: None,
                    clock_rate: 90_000,
                    sample_rate: None,
                    channels: None,
                    width: None,
                    height: None,
                    fps: None,
                    bitrate: None,
                    extradata: CodecExtradata::H264 {
                        sps,
                        pps,
                        avcc: Some(Bytes::copy_from_slice(avcc)),
                    },
                    readiness: TrackReadiness::Ready,
                });
                Ok(Some(FlvIngressOutput::Track(self.tracks.clone())))
            }
            1 => {
                let track =
                    self.find_video_track(CodecId::H264)
                        .ok_or(FlvIngressError::TrackMissing {
                            tag_type: FlvTagType::Video,
                        })?;
                let nalu_payload = tag.payload.slice(5..);
                let annexb = annexb_from_payload(nalu_payload);
                let dts = self.unwrapped_timestamp_ms(tag.timestamp_ms) as i64;
                let pts = dts.saturating_add(i64::from(cts));
                let mut frame = AVFrame::new(
                    track.track_id,
                    MediaKind::Video,
                    CodecId::H264,
                    FrameFormat::CanonicalH26x,
                    pts,
                    dts,
                    Timebase::new(1, 1_000),
                    annexb,
                );
                frame.flags = build_video_frame_flags(frame_type);
                frame.origin = FrameOrigin::Ingest;
                frame.set_source_timestamp(SourceTimestamp::Rtmp(RtmpTimestamp::new(
                    tag.timestamp_ms,
                    self.unwrapped_timestamp_ms(tag.timestamp_ms),
                )));
                Ok(Some(FlvIngressOutput::Frame(Box::new(frame))))
            }
            2 => {
                // AVC end of sequence -> no frame
                Ok(None)
            }
            _ => Err(FlvIngressError::InvalidPacketType {
                tag_type: FlvTagType::Video,
                packet_type,
            }),
        }
    }

    fn process_h265_video_tag(
        &mut self,
        tag: &FlvTag,
        frame_type: u8,
    ) -> Result<Option<FlvIngressOutput>, FlvIngressError> {
        // Legacy H.265 uses the same 5-byte header as H.264 with packet type.
        if tag.payload.len() < 5 {
            return Err(FlvIngressError::PayloadTooShort {
                tag_type: FlvTagType::Video,
            });
        }

        let packet_type = tag.payload[1];
        let cts = sign_extend_i24(u32::from_be_bytes([
            0,
            tag.payload[2],
            tag.payload[3],
            tag.payload[4],
        ]));

        match packet_type {
            0 => {
                let hvcc = tag.payload.slice(5..);
                let track_id = self.video_track_id();
                self.add_or_update_track(TrackInfo {
                    track_id,
                    media_kind: MediaKind::Video,
                    codec: CodecId::H265,
                    aac_rtp_packetization: Default::default(),
                    aac_latm_config_in_band: false,
                    payload_type: None,
                    clock_rate: 90_000,
                    sample_rate: None,
                    channels: None,
                    width: None,
                    height: None,
                    fps: None,
                    bitrate: None,
                    extradata: CodecExtradata::H265 {
                        vps: Vec::new(),
                        sps: Vec::new(),
                        pps: Vec::new(),
                        hvcc: Some(hvcc),
                    },
                    readiness: TrackReadiness::PendingConfig,
                });
                Ok(Some(FlvIngressOutput::Track(self.tracks.clone())))
            }
            1 => {
                let track =
                    self.find_video_track(CodecId::H265)
                        .ok_or(FlvIngressError::TrackMissing {
                            tag_type: FlvTagType::Video,
                        })?;
                let nalu_payload = tag.payload.slice(5..);
                let annexb = annexb_from_payload(nalu_payload);
                let dts = self.unwrapped_timestamp_ms(tag.timestamp_ms) as i64;
                let pts = dts.saturating_add(i64::from(cts));
                let mut frame = AVFrame::new(
                    track.track_id,
                    MediaKind::Video,
                    CodecId::H265,
                    FrameFormat::CanonicalH26x,
                    pts,
                    dts,
                    Timebase::new(1, 1_000),
                    annexb,
                );
                frame.flags = build_video_frame_flags(frame_type);
                frame.origin = FrameOrigin::Ingest;
                frame.set_source_timestamp(SourceTimestamp::Rtmp(RtmpTimestamp::new(
                    tag.timestamp_ms,
                    self.unwrapped_timestamp_ms(tag.timestamp_ms),
                )));
                Ok(Some(FlvIngressOutput::Frame(Box::new(frame))))
            }
            _ => Err(FlvIngressError::InvalidPacketType {
                tag_type: FlvTagType::Video,
                packet_type,
            }),
        }
    }

    fn process_audio_tag(
        &mut self,
        tag: FlvTag,
    ) -> Result<Option<FlvIngressOutput>, FlvIngressError> {
        if tag.payload.is_empty() {
            return Err(FlvIngressError::PayloadTooShort {
                tag_type: FlvTagType::Audio,
            });
        }

        let byte0 = tag.payload[0];
        let sound_format = (byte0 >> 4) & 0x0f;

        if sound_format != 10 {
            return Err(FlvIngressError::UnsupportedCodec {
                tag_type: FlvTagType::Audio,
                codec_id: sound_format,
            });
        }

        if tag.payload.len() < 2 {
            return Err(FlvIngressError::PayloadTooShort {
                tag_type: FlvTagType::Audio,
            });
        }

        let packet_type = tag.payload[1];
        match packet_type {
            0 => {
                let asc_bytes = &tag.payload[2..];
                let asc = AacAudioSpecificConfig::from_bytes(asc_bytes).ok_or(
                    FlvIngressError::ParseFailed {
                        detail: "invalid AAC AudioSpecificConfig",
                    },
                )?;
                let sample_rate = aac_sample_rate_from_index(asc.sampling_frequency_index).ok_or(
                    FlvIngressError::ParseFailed {
                        detail: "unsupported AAC sampling frequency index",
                    },
                )?;
                let channels = aac_channel_count_from_config(asc.channel_configuration).ok_or(
                    FlvIngressError::ParseFailed {
                        detail: "unsupported AAC channel configuration",
                    },
                )?;
                let track_id = self.audio_track_id();
                self.add_or_update_track(TrackInfo {
                    track_id,
                    media_kind: MediaKind::Audio,
                    codec: CodecId::AAC,
                    aac_rtp_packetization: Default::default(),
                    aac_latm_config_in_band: false,
                    payload_type: None,
                    clock_rate: sample_rate,
                    sample_rate: Some(sample_rate),
                    channels: Some(channels),
                    width: None,
                    height: None,
                    fps: None,
                    bitrate: None,
                    extradata: CodecExtradata::AAC {
                        asc: Bytes::copy_from_slice(&asc.to_bytes()),
                    },
                    readiness: TrackReadiness::Ready,
                });
                Ok(Some(FlvIngressOutput::Track(self.tracks.clone())))
            }
            1 => {
                let track =
                    self.find_audio_track(CodecId::AAC)
                        .ok_or(FlvIngressError::TrackMissing {
                            tag_type: FlvTagType::Audio,
                        })?;
                let raw = tag.payload.slice(2..);
                let dts = self.unwrapped_timestamp_ms(tag.timestamp_ms) as i64;
                let mut frame = AVFrame::new(
                    track.track_id,
                    MediaKind::Audio,
                    CodecId::AAC,
                    FrameFormat::AacRaw,
                    dts,
                    dts,
                    Timebase::new(1, 1_000),
                    raw,
                );
                frame.flags = FrameFlags::START_OF_AU | FrameFlags::END_OF_AU;
                frame.origin = FrameOrigin::Ingest;
                frame.set_source_timestamp(SourceTimestamp::Rtmp(RtmpTimestamp::new(
                    tag.timestamp_ms,
                    self.unwrapped_timestamp_ms(tag.timestamp_ms),
                )));
                Ok(Some(FlvIngressOutput::Frame(Box::new(frame))))
            }
            _ => Err(FlvIngressError::InvalidPacketType {
                tag_type: FlvTagType::Audio,
                packet_type,
            }),
        }
    }

    fn unwrapped_timestamp_ms(&mut self, raw: u32) -> u64 {
        let raw_u64 = u64::from(raw);
        // Detect forward wrap: if the raw timestamp jumps back by more than half
        // the 32-bit range, assume a rollover.
        if self.last_raw_timestamp > raw_u64 + (1u64 << 31) {
            self.epoch_offset = self.epoch_offset.wrapping_add(1u64 << 32);
        }
        self.last_raw_timestamp = raw_u64;
        self.epoch_offset.wrapping_add(raw_u64)
    }

    fn video_track_id(&mut self) -> TrackId {
        if self.has_video {
            TrackId(0)
        } else {
            self.video_track_id.unwrap_or_else(|| {
                let id = TrackId(self.next_track_id());
                self.video_track_id = Some(id);
                id
            })
        }
    }

    fn audio_track_id(&mut self) -> TrackId {
        if self.has_audio {
            TrackId(1)
        } else {
            self.audio_track_id.unwrap_or_else(|| {
                let id = TrackId(self.next_track_id());
                self.audio_track_id = Some(id);
                id
            })
        }
    }

    fn next_track_id(&mut self) -> u32 {
        let id = self.next_track_id;
        self.next_track_id += 1;
        id
    }

    fn add_or_update_track(&mut self, track: TrackInfo) {
        if let Some(existing) = self
            .tracks
            .iter_mut()
            .find(|t| t.track_id == track.track_id && t.media_kind == track.media_kind)
        {
            *existing = track;
        } else {
            self.tracks.push(track);
        }
    }

    fn find_video_track(&self, codec: CodecId) -> Option<TrackInfo> {
        self.tracks
            .iter()
            .find(|t| t.media_kind == MediaKind::Video && t.codec == codec)
            .cloned()
    }

    fn find_audio_track(&self, codec: CodecId) -> Option<TrackInfo> {
        self.tracks
            .iter()
            .find(|t| t.media_kind == MediaKind::Audio && t.codec == codec)
            .cloned()
    }
}

fn sign_extend_i24(value: u32) -> i32 {
    // FLV composition time is a signed 24-bit big-endian value.
    let shifted = (value << 8) as i32;
    shifted >> 8
}

fn build_video_frame_flags(frame_type: u8) -> FrameFlags {
    match frame_type {
        1 => FrameFlags::KEY | FrameFlags::START_OF_AU | FrameFlags::END_OF_AU,
        2..=4 => FrameFlags::START_OF_AU | FrameFlags::END_OF_AU,
        5 => FrameFlags::KEY | FrameFlags::GENERATED,
        _ => FrameFlags::empty(),
    }
}

fn aac_sample_rate_from_index(index: u8) -> Option<u32> {
    match index {
        0 => Some(96_000),
        1 => Some(88_200),
        2 => Some(64_000),
        3 => Some(48_000),
        4 => Some(44_100),
        5 => Some(32_000),
        6 => Some(24_000),
        7 => Some(22_050),
        8 => Some(16_000),
        9 => Some(12_000),
        10 => Some(11_025),
        11 => Some(8_000),
        12 => Some(7_350),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::CodecId;

    #[test]
    fn h264_sequence_header_creates_track() {
        let sps = Bytes::from_static(&[
            0x67, 0x42, 0x00, 0x1f, 0x96, 0x54, 0x05, 0x01, 0xed, 0x00, 0xf0, 0x88, 0x45, 0x80,
        ]);
        let pps = Bytes::from_static(&[0x68, 0xce, 0x06, 0xe2]);
        let avcc = build_test_avcc(&sps, &pps);

        let mut payload = vec![0x17, 0x00, 0x00, 0x00, 0x00];
        payload.extend_from_slice(&avcc);

        let tag = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 0,
            payload: Bytes::from(payload),
        };

        let mut ingress = FlvIngress::new();
        let out = ingress.process_tag(tag).unwrap().unwrap();
        let FlvIngressOutput::Track(tracks) = out else {
            panic!("expected track");
        };
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].codec, CodecId::H264);
        assert!(matches!(tracks[0].extradata, CodecExtradata::H264 { .. }));
    }

    #[test]
    fn aac_sequence_header_creates_track() {
        let asc = [0x12, 0x10]; // 44.1 kHz, 2 channels
        let mut payload = vec![0xaf, 0x00];
        payload.extend_from_slice(&asc);

        let tag = FlvTag {
            tag_type: FlvTagType::Audio,
            timestamp_ms: 0,
            payload: Bytes::from(payload),
        };

        let mut ingress = FlvIngress::new();
        let out = ingress.process_tag(tag).unwrap().unwrap();
        let FlvIngressOutput::Track(tracks) = out else {
            panic!("expected track");
        };
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].codec, CodecId::AAC);
        assert_eq!(tracks[0].sample_rate, Some(44_100));
        assert_eq!(tracks[0].channels, Some(2));
    }

    #[test]
    fn h264_nalu_converts_to_annexb_frame() {
        let sps = Bytes::from_static(&[
            0x67, 0x42, 0x00, 0x1f, 0x96, 0x54, 0x05, 0x01, 0xed, 0x00, 0xf0, 0x88, 0x45, 0x80,
        ]);
        let pps = Bytes::from_static(&[0x68, 0xce, 0x06, 0xe2]);
        let avcc = build_test_avcc(&sps, &pps);

        let mut seq_payload = vec![0x17, 0x00, 0x00, 0x00, 0x00];
        seq_payload.extend_from_slice(&avcc);
        let seq = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 0,
            payload: Bytes::from(seq_payload),
        };

        // One NALU: 00 00 00 01 65 01 02 03
        let nalu = vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x01, 0x02, 0x03];
        let mut frame_payload = vec![0x17, 0x01, 0x00, 0x00, 0x00];
        frame_payload.extend_from_slice(&nalu);
        let frame_tag = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 33,
            payload: Bytes::from(frame_payload),
        };

        let mut ingress = FlvIngress::new();
        let _ = ingress.process_tag(seq).unwrap();
        let out = ingress.process_tag(frame_tag).unwrap().unwrap();
        let FlvIngressOutput::Frame(frame) = out else {
            panic!("expected frame");
        };
        assert_eq!(frame.codec, CodecId::H264);
        assert!(frame.is_key_frame());
        assert_eq!(frame.dts, 33);
        assert_eq!(frame.pts, 33);
        assert_eq!(frame.format, FrameFormat::CanonicalH26x);
        assert!(frame.payload.starts_with(&[0x00, 0x00, 0x00, 0x01, 0x65]));
    }

    fn build_test_avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[
            1,      // configurationVersion
            sps[1], // profile
            sps[2], // compatibility
            sps[3], // level
            0xFF,   // reserved + lengthSizeMinusOne(3)
            0xE0 | 1,
        ]);
        out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        out.extend_from_slice(sps);
        out.push(1);
        out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        out.extend_from_slice(pps);
        out
    }
}
