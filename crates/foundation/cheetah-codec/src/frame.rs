use crate::prelude::*;
use bitflags::bitflags;
use bytes::Bytes;
use smallvec::SmallVec;

use crate::time::Timebase;
use crate::track::{CodecId, MediaKind, TrackId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    CanonicalH26x,
    CanonicalAv1Obu,
    CanonicalVp8Frame,
    CanonicalVp9Frame,
    MjpegFrame,
    AacRaw,
    AdpcmPacket,
    OpusPacket,
    G711Packet,
    Mp2Frame,
    Mp3Frame,
    DataPacket,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOrigin {
    Ingest,
    Relay,
    Generated,
    Recovered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpRtcpMapping {
    pub lsr: u32,
    pub arrival_unix_micros: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpTimestamp {
    pub raw_timestamp: u32,
    pub unwrapped_timestamp: u64,
    pub epoch_offset: i64,
    pub sequence_number: Option<u16>,
    pub rtcp_mapping: Option<RtpRtcpMapping>,
}

impl RtpTimestamp {
    pub fn new(raw_timestamp: u32, unwrapped_timestamp: u64) -> Self {
        Self {
            raw_timestamp,
            unwrapped_timestamp,
            epoch_offset: saturating_epoch_offset(unwrapped_timestamp, raw_timestamp),
            sequence_number: None,
            rtcp_mapping: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtmpTimestamp {
    pub raw_timestamp_ms: u32,
    pub unwrapped_timestamp_ms: u64,
    pub epoch_offset_ms: i64,
}

impl RtmpTimestamp {
    pub fn new(raw_timestamp_ms: u32, unwrapped_timestamp_ms: u64) -> Self {
        Self {
            raw_timestamp_ms,
            unwrapped_timestamp_ms,
            epoch_offset_ms: saturating_epoch_offset(unwrapped_timestamp_ms, raw_timestamp_ms),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTimestamp {
    Rtp(RtpTimestamp),
    Rtmp(RtmpTimestamp),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameSideData {
    ParameterSetDigest(u64),
    SequenceNumber(u64),
    SourceTimestamp(SourceTimestamp),
    Metadata { key: String, value: String },
    Opaque(Bytes),
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct FrameFlags: u32 {
        const KEY = 1 << 0;
        const CONFIG = 1 << 1;
        const START_OF_AU = 1 << 2;
        const END_OF_AU = 1 << 3;
        const B_FRAME = 1 << 4;
        const NON_PICTURE = 1 << 5;
        const DISCONTINUITY = 1 << 6;
        const GENERATED = 1 << 7;
        const CORRUPTED = 1 << 8;
        const DROPPABLE = 1 << 9;
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum FrameTimingError {
    #[error("track {track_id:?} has invalid timebase {timebase_num}/{timebase_den}")]
    InvalidTimebase {
        track_id: TrackId,
        timebase_num: u32,
        timebase_den: u32,
    },
    #[error("track {track_id:?} has negative duration {duration}")]
    NegativeDuration { track_id: TrackId, duration: i64 },
    #[error("track {track_id:?} duration micros overflow for duration {duration}")]
    DurationMicrosOverflow { track_id: TrackId, duration: i64 },
    #[error("video track {track_id:?} has pts < dts without B-frame flag: pts={pts}, dts={dts}")]
    NegativeCompositionWithoutBFrame {
        track_id: TrackId,
        pts: i64,
        dts: i64,
    },
}

#[derive(Debug, Clone)]
pub struct AVFrame {
    pub track_id: TrackId,
    pub media_kind: MediaKind,
    pub codec: CodecId,
    pub format: FrameFormat,
    /// Decoding/presentation timeline expressed in `timebase` ticks.
    /// Invariant: for non-B video frames, `pts >= dts`.
    pub pts: i64,
    pub dts: i64,
    /// Canonical media time unit for this frame. Must be non-zero.
    pub timebase: Timebase,
    /// `pts/dts` converted to microseconds for cross-protocol scheduling/logging.
    pub pts_us: i64,
    pub dts_us: i64,
    /// Frame duration expressed in `timebase` ticks and microseconds.
    /// `duration == 0` means "unknown/unspecified".
    pub duration: i64,
    pub duration_us: i64,
    /// Key/random-access, config frame, discontinuity and corruption semantics.
    pub flags: FrameFlags,
    pub payload: Bytes,
    pub side_data: SmallVec<[FrameSideData; 4]>,
    pub origin: FrameOrigin,
}

impl AVFrame {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        track_id: TrackId,
        media_kind: MediaKind,
        codec: CodecId,
        format: FrameFormat,
        pts: i64,
        dts: i64,
        timebase: Timebase,
        payload: Bytes,
    ) -> Self {
        let pts_us = Timebase::to_micros(timebase, pts);
        let dts_us = Timebase::to_micros(timebase, dts);
        Self {
            track_id,
            media_kind,
            codec,
            format,
            pts,
            dts,
            timebase,
            pts_us,
            dts_us,
            duration: 0,
            duration_us: 0,
            flags: FrameFlags::empty(),
            payload,
            side_data: SmallVec::new(),
            origin: FrameOrigin::Ingest,
        }
    }

    pub fn is_key_frame(&self) -> bool {
        self.flags.contains(FrameFlags::KEY)
    }

    pub fn mark_discontinuity(&mut self) {
        self.flags.insert(FrameFlags::DISCONTINUITY);
    }

    pub fn set_source_timestamp(&mut self, source_timestamp: SourceTimestamp) {
        if let Some(existing) = self
            .side_data
            .iter_mut()
            .find(|entry| matches!(entry, FrameSideData::SourceTimestamp(_)))
        {
            *existing = FrameSideData::SourceTimestamp(source_timestamp);
            return;
        }
        self.side_data
            .push(FrameSideData::SourceTimestamp(source_timestamp));
    }

    pub fn source_timestamp(&self) -> Option<SourceTimestamp> {
        self.side_data.iter().find_map(|entry| {
            let FrameSideData::SourceTimestamp(source) = entry else {
                return None;
            };
            Some(*source)
        })
    }

    pub fn with_duration(mut self, duration: i64) -> Result<Self, FrameTimingError> {
        self.set_duration(duration)?;
        Ok(self)
    }

    pub fn set_duration(&mut self, duration: i64) -> Result<(), FrameTimingError> {
        if duration < 0 {
            return Err(FrameTimingError::NegativeDuration {
                track_id: self.track_id,
                duration,
            });
        }
        self.ensure_valid_timebase()?;
        self.duration = duration;
        self.duration_us = self.duration_to_micros_checked(duration)?;
        Ok(())
    }

    pub fn composition_time(&self) -> i64 {
        self.pts.saturating_sub(self.dts)
    }

    pub fn composition_time_us(&self) -> i64 {
        self.pts_us.saturating_sub(self.dts_us)
    }

    pub fn validate_media_timing(&self) -> Result<(), FrameTimingError> {
        self.ensure_valid_timebase()?;
        if self.duration < 0 {
            return Err(FrameTimingError::NegativeDuration {
                track_id: self.track_id,
                duration: self.duration,
            });
        }
        let _ = self.duration_to_micros_checked(self.duration)?;
        if self.media_kind == MediaKind::Video
            && self.pts < self.dts
            && !self.flags.contains(FrameFlags::B_FRAME)
        {
            return Err(FrameTimingError::NegativeCompositionWithoutBFrame {
                track_id: self.track_id,
                pts: self.pts,
                dts: self.dts,
            });
        }
        Ok(())
    }

    fn ensure_valid_timebase(&self) -> Result<(), FrameTimingError> {
        if self.timebase.num == 0 || self.timebase.den == 0 {
            return Err(FrameTimingError::InvalidTimebase {
                track_id: self.track_id,
                timebase_num: self.timebase.num,
                timebase_den: self.timebase.den,
            });
        }
        Ok(())
    }

    fn duration_to_micros_checked(&self, duration: i64) -> Result<i64, FrameTimingError> {
        let scaled = i128::from(duration)
            .checked_mul(i128::from(self.timebase.num))
            .and_then(|v| v.checked_mul(1_000_000_i128))
            .ok_or(FrameTimingError::DurationMicrosOverflow {
                track_id: self.track_id,
                duration,
            })?;
        let micros = scaled / i128::from(self.timebase.den);
        i64::try_from(micros).map_err(|_| FrameTimingError::DurationMicrosOverflow {
            track_id: self.track_id,
            duration,
        })
    }
}

fn saturating_epoch_offset(unwrapped_timestamp: u64, raw_timestamp: u32) -> i64 {
    let delta = i128::from(unwrapped_timestamp).saturating_sub(i128::from(raw_timestamp));
    delta.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_updates_micros_and_prevents_negative_values() {
        let tb = Timebase::new(1, 1_000);
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            10,
            10,
            tb,
            Bytes::from_static(b"payload"),
        )
        .with_duration(33)
        .expect("positive duration should be accepted");
        assert_eq!(frame.duration, 33);
        assert_eq!(frame.duration_us, 33_000);

        let err = frame
            .with_duration(-1)
            .expect_err("negative duration must fail");
        assert!(matches!(err, FrameTimingError::NegativeDuration { .. }));
    }

    #[test]
    fn composition_time_is_non_negative_for_normal_video_paths() {
        let tb = Timebase::new(1, 1_000);
        let frame = AVFrame::new(
            TrackId(2),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            20,
            10,
            tb,
            Bytes::from_static(b"payload"),
        );
        assert_eq!(frame.composition_time_us(), 10_000);
    }

    #[test]
    fn detects_negative_composition_without_b_frame_flag() {
        let tb = Timebase::new(1, 1_000);
        let frame = AVFrame::new(
            TrackId(3),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            10,
            20,
            tb,
            Bytes::from_static(b"payload"),
        );

        let err = frame
            .validate_media_timing()
            .expect_err("pts < dts without B-frame flag should be rejected");
        assert!(matches!(
            err,
            FrameTimingError::NegativeCompositionWithoutBFrame {
                track_id: TrackId(3),
                ..
            }
        ));
    }

    #[test]
    fn source_timestamp_side_data_can_be_set_and_read() {
        let tb = Timebase::new(1, 1_000);
        let mut frame = AVFrame::new(
            TrackId(7),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            90,
            60,
            tb,
            Bytes::from_static(b"payload"),
        );

        let mut rtp = RtpTimestamp::new(4_321, 4_321);
        rtp.sequence_number = Some(77);
        rtp.rtcp_mapping = Some(RtpRtcpMapping {
            lsr: 0x0011_2233,
            arrival_unix_micros: 123_456,
        });
        frame.set_source_timestamp(SourceTimestamp::Rtp(rtp));

        assert_eq!(frame.source_timestamp(), Some(SourceTimestamp::Rtp(rtp)));
        assert_eq!(
            frame.side_data.len(),
            1,
            "source timestamp should be stored as a single side-data item"
        );
    }

    #[test]
    fn source_timestamp_setter_replaces_existing_entry() {
        let tb = Timebase::new(1, 1_000);
        let mut frame = AVFrame::new(
            TrackId(8),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            10,
            10,
            tb,
            Bytes::from_static(b"payload"),
        );

        frame.set_source_timestamp(SourceTimestamp::Rtmp(RtmpTimestamp::new(30, 30)));
        frame.set_source_timestamp(SourceTimestamp::Rtmp(RtmpTimestamp::new(60, 90)));

        assert_eq!(
            frame.source_timestamp(),
            Some(SourceTimestamp::Rtmp(RtmpTimestamp::new(60, 90)))
        );
        assert_eq!(
            frame.side_data.len(),
            1,
            "setting source timestamp twice must replace the previous entry"
        );
    }
}
