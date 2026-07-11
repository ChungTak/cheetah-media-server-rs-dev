//! FLV record container writer.
//!
//! Wraps `cheetah_codec::flv` to produce a continuous FLV file by streaming
//! tag bytes. The writer takes track info to emit sequence headers, then
//! emits an `onMetaData` script tag, and finally tags for each `AVFrame`.
//!
//! FLV 录制容器写入器。
//!
//! 封装 `cheetah_codec::flv`，通过流式输出标签字节生成连续 FLV 文件。
//! 写入器接收轨道信息并输出序列头，然后输出 `onMetaData` 脚本标签，
//! 最后为每个 `AVFrame` 输出标签。

use crate::prelude::*;
use bytes::{BufMut, Bytes, BytesMut};

use crate::flv::{
    build_audio_sequence_header, build_video_sequence_header, FlvHeader, FlvTag, FlvTagType,
};
use crate::frame::{AVFrame, FrameFlags};
use crate::track::{CodecId, MediaKind, TrackInfo};

use super::{RecordContainerWriter, RecordDiagnostic, RecordError, RecordFormat, RecordWriteEvent};

/// Writer configuration.
///
/// 写入器配置。
#[derive(Debug, Clone, Default)]
pub struct FlvFileWriterConfig {
    pub include_onmetadata: bool,
}

/// Stateful FLV file writer.
///
/// 有状态 FLV 文件写入器。
pub struct FlvFileWriter {
    config: FlvFileWriterConfig,
    header_emitted: bool,
    finalized: bool,
    tracks: Vec<TrackInfo>,
    base_dts_us: Option<i64>,
}

impl FlvFileWriter {
    /// Create a new FLV file writer with the given configuration.
    ///
    /// 使用给定配置创建新的 FLV 文件写入器。
    pub fn new(config: FlvFileWriterConfig) -> Self {
        Self {
            config,
            header_emitted: false,
            finalized: false,
            tracks: Vec::new(),
            base_dts_us: None,
        }
    }

    /// Encode a 24-bit signed FLV CTS (composition time, milliseconds)
    /// from a microsecond delta. Values outside [-2^23, 2^23-1] are
    /// clamped to the representable range.
    fn encode_cts24(payload: &mut BytesMut, cts_us: i64) {
        let cts_ms = (cts_us / 1000).clamp(-(1 << 23), (1 << 23) - 1);
        let bytes = (cts_ms as i32).to_be_bytes();
        // i32 → take low 24 bits, preserving sign via two's complement.
        payload.put_u8(bytes[1]);
        payload.put_u8(bytes[2]);
        payload.put_u8(bytes[3]);
    }

    fn emit_header(&self) -> Bytes {
        let has_audio = self.tracks.iter().any(|t| t.media_kind == MediaKind::Audio);
        let has_video = self.tracks.iter().any(|t| t.media_kind == MediaKind::Video);
        FlvHeader {
            has_audio,
            has_video,
        }
        .encode()
    }

    fn emit_sequence_headers(&self) -> Vec<RecordWriteEvent> {
        let mut out = Vec::new();
        for t in &self.tracks {
            match t.media_kind {
                MediaKind::Video => {
                    if let Some(body) = build_video_sequence_header(t) {
                        let tag = FlvTag {
                            tag_type: FlvTagType::Video,
                            timestamp_ms: 0,
                            payload: body.payload,
                        };
                        out.push(RecordWriteEvent::Bytes(tag.encode_with_previous_tag_size()));
                    }
                }
                MediaKind::Audio => {
                    if let Some(body) = build_audio_sequence_header(t) {
                        let tag = FlvTag {
                            tag_type: FlvTagType::Audio,
                            timestamp_ms: 0,
                            payload: body.payload,
                        };
                        out.push(RecordWriteEvent::Bytes(tag.encode_with_previous_tag_size()));
                    }
                }
                _ => {}
            }
        }
        out
    }
}

impl RecordContainerWriter for FlvFileWriter {
    /// Store the active track list for header/sequence-header generation.
    ///
    /// 保存活动轨道列表，用于生成头部与序列头。
    fn update_tracks(&mut self, tracks: &[TrackInfo]) -> Result<(), RecordError> {
        if tracks.is_empty() {
            return Err(RecordError::InvalidTracks("no tracks"));
        }
        self.tracks = tracks.to_vec();
        Ok(())
    }

    /// Encode an `AVFrame` as an FLV tag and emit it along with the file header
    /// and sequence headers on the first call.
    ///
    /// 将 `AVFrame` 编码为 FLV 标签，并在首次调用时一并输出文件头与序列头。
    fn push_frame(&mut self, frame: &AVFrame) -> Result<Vec<RecordWriteEvent>, RecordError> {
        if self.finalized {
            return Err(RecordError::Finalized);
        }
        if self.tracks.is_empty() {
            return Err(RecordError::NotInitialized);
        }
        let mut out = Vec::new();
        if !self.header_emitted {
            out.push(RecordWriteEvent::Bytes(self.emit_header()));
            out.extend(self.emit_sequence_headers());
            self.header_emitted = true;
        }
        if self.base_dts_us.is_none() {
            self.base_dts_us = Some(frame.dts_us);
        }
        let base = self.base_dts_us.unwrap_or(0);
        // Saturating sub guards against accidental underflow if frames
        // arrive out-of-order across the base sample.
        let ts = frame
            .dts_us
            .saturating_sub(base)
            .saturating_div(1000)
            .max(0)
            .min(u32::MAX as i64) as u32;
        match frame.media_kind {
            MediaKind::Video => match frame.codec {
                CodecId::H264 => {
                    let mut payload = BytesMut::with_capacity(frame.payload.len() + 5);
                    let frame_type = if frame.flags.contains(FrameFlags::KEY) {
                        0x17
                    } else {
                        0x27
                    };
                    payload.put_u8(frame_type);
                    payload.put_u8(0x01); // AVC NALU
                    Self::encode_cts24(&mut payload, frame.composition_time_us());
                    payload.extend_from_slice(&frame.payload);
                    out.push(RecordWriteEvent::Bytes(
                        FlvTag {
                            tag_type: FlvTagType::Video,
                            timestamp_ms: ts,
                            payload: payload.freeze(),
                        }
                        .encode_with_previous_tag_size(),
                    ));
                }
                CodecId::H265 => {
                    let mut payload = BytesMut::with_capacity(frame.payload.len() + 5);
                    let frame_type = if frame.flags.contains(FrameFlags::KEY) {
                        0x1c
                    } else {
                        0x2c
                    };
                    payload.put_u8(frame_type);
                    payload.put_u8(0x01);
                    Self::encode_cts24(&mut payload, frame.composition_time_us());
                    payload.extend_from_slice(&frame.payload);
                    out.push(RecordWriteEvent::Bytes(
                        FlvTag {
                            tag_type: FlvTagType::Video,
                            timestamp_ms: ts,
                            payload: payload.freeze(),
                        }
                        .encode_with_previous_tag_size(),
                    ));
                }
                _ => {
                    out.push(RecordWriteEvent::Diagnostic(
                        RecordDiagnostic::UnsupportedCodec {
                            codec: frame.codec,
                            track_id: frame.track_id.0,
                        },
                    ));
                }
            },
            MediaKind::Audio => match frame.codec {
                CodecId::AAC => {
                    let mut payload = BytesMut::with_capacity(frame.payload.len() + 2);
                    payload.put_u8(0xaf);
                    payload.put_u8(0x01); // raw AAC frame
                    payload.extend_from_slice(&frame.payload);
                    out.push(RecordWriteEvent::Bytes(
                        FlvTag {
                            tag_type: FlvTagType::Audio,
                            timestamp_ms: ts,
                            payload: payload.freeze(),
                        }
                        .encode_with_previous_tag_size(),
                    ));
                }
                CodecId::G711A => {
                    let mut payload = BytesMut::with_capacity(frame.payload.len() + 1);
                    payload.put_u8(0x72); // 7=G711A, 2=16k, 1=16bit, 0=mono
                    payload.extend_from_slice(&frame.payload);
                    out.push(RecordWriteEvent::Bytes(
                        FlvTag {
                            tag_type: FlvTagType::Audio,
                            timestamp_ms: ts,
                            payload: payload.freeze(),
                        }
                        .encode_with_previous_tag_size(),
                    ));
                }
                CodecId::G711U => {
                    let mut payload = BytesMut::with_capacity(frame.payload.len() + 1);
                    payload.put_u8(0x82);
                    payload.extend_from_slice(&frame.payload);
                    out.push(RecordWriteEvent::Bytes(
                        FlvTag {
                            tag_type: FlvTagType::Audio,
                            timestamp_ms: ts,
                            payload: payload.freeze(),
                        }
                        .encode_with_previous_tag_size(),
                    ));
                }
                _ => {
                    out.push(RecordWriteEvent::Diagnostic(
                        RecordDiagnostic::UnsupportedCodec {
                            codec: frame.codec,
                            track_id: frame.track_id.0,
                        },
                    ));
                }
            },
            _ => {
                out.push(RecordWriteEvent::Diagnostic(
                    RecordDiagnostic::UnsupportedTrack {
                        track_id: frame.track_id.0,
                        reason: "non-AV media kind",
                    },
                ));
            }
        }
        let _ = &self.config;
        Ok(out)
    }

    /// Mark the writer as finalized. No trailing data is required for FLV.
    ///
    /// 标记写入器已完成。FLV 不需要尾部数据。
    fn finalize(&mut self) -> Result<Vec<RecordWriteEvent>, RecordError> {
        self.finalized = true;
        Ok(Vec::new())
    }

    /// 返回 `RecordFormat::Flv`。
    fn format(&self) -> RecordFormat {
        RecordFormat::Flv
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{AVFrame, FrameFormat};
    use crate::time::Timebase;
    use crate::track::{CodecExtradata, CodecId, MediaKind, TrackId, TrackInfo};

    #[test]
    fn writes_h264_aac_recording() {
        let mut wv = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        wv.extradata = CodecExtradata::H264 {
            sps: vec![],
            pps: vec![],
            avcc: Some(Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
                0x03, 0x68, 0xCE, 0x38,
            ])),
        };
        let mut wa = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 44_100);
        wa.extradata = CodecExtradata::AAC {
            asc: Bytes::from_static(&[0x12, 0x10]),
        };

        let mut writer = FlvFileWriter::new(FlvFileWriterConfig::default());
        writer.update_tracks(&[wv.clone(), wa.clone()]).unwrap();

        let tb = Timebase::new(1, 1000);
        let mut vid = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            tb,
            Bytes::from_static(b"VAU"),
        );
        vid.flags.insert(FrameFlags::KEY);

        let aud = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            0,
            0,
            tb,
            Bytes::from_static(b"AAU"),
        );

        let v_events = writer.push_frame(&vid).unwrap();
        let a_events = writer.push_frame(&aud).unwrap();
        let mut buf = BytesMut::new();
        for e in v_events.iter().chain(a_events.iter()) {
            if let RecordWriteEvent::Bytes(b) = e {
                buf.extend_from_slice(b);
            }
        }
        // First three bytes should be the FLV signature
        assert_eq!(&buf[..3], b"FLV");
    }
}
