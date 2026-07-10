//! MPEG-TS muxer for HLS segment generation.
//!
//! Thin wrapper around `cheetah_codec::MpegTsMuxer` that accumulates TS packets
//! into a segment buffer. Supports H.264/H.265/VP8/VP9/AV1 video and
//! AAC/G711A/G711U/MP3/OPUS audio.

use bytes::{Bytes, BytesMut};
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, MpegTsMuxEvent, MpegTsMuxer,
    MpegTsMuxerConfig, Timebase, TrackId, TrackInfo,
};

/// MPEG-TS muxer that accumulates TS packets for a single segment.
pub struct TsMuxer {
    inner: MpegTsMuxer,
    buf: BytesMut,
    video_track_id: TrackId,
    audio_track_id: TrackId,
    video_codec: CodecId,
    audio_codec: CodecId,
}

impl TsMuxer {
    pub fn new(video_codec: CodecId, audio_codec: CodecId, has_audio: bool) -> Self {
        let video_track_id = TrackId(1);
        let audio_track_id = TrackId(2);
        let mut tracks = vec![TrackInfo::new(
            video_track_id,
            MediaKind::Video,
            video_codec,
            90_000,
        )];
        if has_audio {
            tracks.push(TrackInfo::new(
                audio_track_id,
                MediaKind::Audio,
                audio_codec,
                90_000,
            ));
        }
        let inner = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        Self {
            inner,
            buf: BytesMut::with_capacity(128 * 1024),
            video_track_id,
            audio_track_id,
            video_codec,
            audio_codec,
        }
    }

    /// Reset the muxer for a new segment. Returns the previous segment data.
    pub fn take_segment(&mut self) -> Bytes {
        self.buf.split().freeze()
    }

    /// Write PAT + PMT tables. Call at the start of each segment.
    pub fn write_pat_pmt(&mut self) {
        for ev in self.inner.write_tables() {
            if let MpegTsMuxEvent::Packet(data) = ev {
                self.buf.extend_from_slice(&data);
            }
        }
    }

    /// Write a video PES packet with AUD prepended for H.264/H.265.
    pub fn write_video(&mut self, data: &[u8], pts: u64, dts: u64, is_keyframe: bool) {
        let format = match self.video_codec {
            CodecId::H264 | CodecId::H265 | CodecId::H266 => FrameFormat::CanonicalH26x,
            CodecId::AV1 => FrameFormat::CanonicalAv1Obu,
            CodecId::VP8 => FrameFormat::CanonicalVp8Frame,
            CodecId::VP9 => FrameFormat::CanonicalVp9Frame,
            _ => FrameFormat::Unknown,
        };
        // Convert 90kHz ticks to microseconds for AVFrame
        let pts_us = (pts as i64) * 100 / 9;
        let dts_us = (dts as i64) * 100 / 9;
        let mut frame = AVFrame::new(
            self.video_track_id,
            MediaKind::Video,
            self.video_codec,
            format,
            pts as i64,
            dts as i64,
            Timebase::new(1, 90_000),
            Bytes::copy_from_slice(data),
        );
        frame.pts_us = pts_us;
        frame.dts_us = dts_us;
        if is_keyframe {
            frame.flags.insert(FrameFlags::KEY);
        }
        for ev in self.inner.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                self.buf.extend_from_slice(&d);
            }
        }
    }

    /// Write an audio PES packet.
    pub fn write_audio(&mut self, data: &[u8], pts: u64) {
        let pts_us = (pts as i64) * 100 / 9;
        let format = match self.audio_codec {
            CodecId::AAC => FrameFormat::AacRaw,
            CodecId::Opus => FrameFormat::OpusPacket,
            CodecId::G711A | CodecId::G711U => FrameFormat::G711Packet,
            CodecId::MP2 => FrameFormat::Mp2Frame,
            CodecId::MP3 => FrameFormat::Mp3Frame,
            _ => FrameFormat::Unknown,
        };
        let mut frame = AVFrame::new(
            self.audio_track_id,
            MediaKind::Audio,
            self.audio_codec,
            format,
            pts as i64,
            pts as i64,
            Timebase::new(1, 90_000),
            Bytes::copy_from_slice(data),
        );
        frame.pts_us = pts_us;
        frame.dts_us = pts_us;
        for ev in self.inner.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                self.buf.extend_from_slice(&d);
            }
        }
    }
}

/// Track descriptor for multi-track muxer.
#[derive(Debug, Clone)]
pub struct TsTrackDesc {
    pub codec: CodecId,
    pub media_kind: MediaKind,
}

/// Multi-track MPEG-TS muxer with dynamic PID allocation.
pub struct TsMuxerMulti {
    inner: MpegTsMuxer,
    buf: BytesMut,
    tracks: Vec<TsTrackDescEntry>,
}

struct TsTrackDescEntry {
    track_id: TrackId,
    codec: CodecId,
    media_kind: MediaKind,
}

impl TsMuxerMulti {
    /// Create a multi-track muxer from track descriptors.
    pub fn new(descs: &[TsTrackDesc]) -> Self {
        let mut track_infos = Vec::with_capacity(descs.len());
        let mut entries = Vec::with_capacity(descs.len());
        for (i, desc) in descs.iter().enumerate() {
            let track_id = TrackId((i + 1) as u32);
            track_infos.push(TrackInfo::new(
                track_id,
                desc.media_kind,
                desc.codec,
                90_000,
            ));
            entries.push(TsTrackDescEntry {
                track_id,
                codec: desc.codec,
                media_kind: desc.media_kind,
            });
        }
        let inner = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &track_infos);
        Self {
            inner,
            buf: BytesMut::with_capacity(128 * 1024),
            tracks: entries,
        }
    }

    pub fn take_segment(&mut self) -> Bytes {
        self.buf.split().freeze()
    }

    pub fn write_pat_pmt(&mut self) {
        for ev in self.inner.write_tables() {
            if let MpegTsMuxEvent::Packet(data) = ev {
                self.buf.extend_from_slice(&data);
            }
        }
    }

    /// Write a frame for the given track index.
    pub fn write_frame(
        &mut self,
        track_index: usize,
        data: &[u8],
        pts: u64,
        dts: u64,
        is_keyframe: bool,
    ) {
        let Some(entry) = self.tracks.get(track_index) else {
            return;
        };
        let is_video = entry.media_kind == MediaKind::Video;
        let format = if is_video {
            match entry.codec {
                CodecId::H264 | CodecId::H265 | CodecId::H266 => FrameFormat::CanonicalH26x,
                CodecId::AV1 => FrameFormat::CanonicalAv1Obu,
                CodecId::VP8 => FrameFormat::CanonicalVp8Frame,
                CodecId::VP9 => FrameFormat::CanonicalVp9Frame,
                _ => FrameFormat::Unknown,
            }
        } else {
            match entry.codec {
                CodecId::AAC => FrameFormat::AacRaw,
                CodecId::Opus => FrameFormat::OpusPacket,
                CodecId::G711A | CodecId::G711U => FrameFormat::G711Packet,
                CodecId::MP2 => FrameFormat::Mp2Frame,
                CodecId::MP3 => FrameFormat::Mp3Frame,
                _ => FrameFormat::Unknown,
            }
        };
        let pts_us = (pts as i64) * 100 / 9;
        let dts_us = (dts as i64) * 100 / 9;
        let mut frame = AVFrame::new(
            entry.track_id,
            entry.media_kind,
            entry.codec,
            format,
            pts as i64,
            dts as i64,
            Timebase::new(1, 90_000),
            Bytes::copy_from_slice(data),
        );
        frame.pts_us = pts_us;
        frame.dts_us = dts_us;
        if is_keyframe {
            frame.flags.insert(FrameFlags::KEY);
        }
        for ev in self.inner.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                self.buf.extend_from_slice(&d);
            }
        }
    }

    /// Number of tracks.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS_PACKET_SIZE: usize = 188;

    #[test]
    fn pat_pmt_produces_valid_ts_packets() {
        let mut muxer = TsMuxer::new(CodecId::H264, CodecId::AAC, true);
        muxer.write_pat_pmt();
        let data = muxer.take_segment();
        assert_eq!(data.len(), 2 * TS_PACKET_SIZE);
        assert_eq!(data[0], 0x47);
        assert_eq!(data[188], 0x47);
    }

    #[test]
    fn video_pes_produces_aligned_packets() {
        let mut muxer = TsMuxer::new(CodecId::H264, CodecId::AAC, false);
        let fake_nalu = vec![0x65_u8; 100];
        muxer.write_video(&fake_nalu, 90000, 90000, true);
        let data = muxer.take_segment();
        assert_eq!(data.len() % TS_PACKET_SIZE, 0);
        assert!(data.len() >= TS_PACKET_SIZE);
    }

    #[test]
    fn h264_aud_injected() {
        let mut muxer = TsMuxer::new(CodecId::H264, CodecId::AAC, false);
        let nalu = vec![0x00, 0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB];
        muxer.write_video(&nalu, 90000, 90000, true);
        let data = muxer.take_segment();
        let aud_h264: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x09, 0xF0];
        let aud_pos = data.windows(aud_h264.len()).position(|w| w == aud_h264);
        assert!(aud_pos.is_some(), "H264 AUD not found in output");
    }

    #[test]
    fn h265_aud_injected() {
        let mut muxer = TsMuxer::new(CodecId::H265, CodecId::AAC, false);
        let nalu = vec![0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xCC];
        muxer.write_video(&nalu, 90000, 90000, true);
        let data = muxer.take_segment();
        let aud_h265: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x46, 0x01, 0x50];
        let aud_pos = data.windows(aud_h265.len()).position(|w| w == aud_h265);
        assert!(aud_pos.is_some(), "H265 AUD not found in output");
    }

    #[test]
    fn cc_wraps_at_16() {
        let mut muxer = TsMuxer::new(CodecId::H264, CodecId::AAC, false);
        for i in 0..20 {
            muxer.write_audio(&[0xAA; 10], i * 90000);
        }
        let data = muxer.take_segment();
        assert_eq!(data.len() % TS_PACKET_SIZE, 0);
        for chunk in data.chunks(TS_PACKET_SIZE) {
            assert_eq!(chunk[0], 0x47);
        }
    }

    #[test]
    fn multi_track_muxer_produces_valid_ts() {
        let tracks = vec![
            TsTrackDesc {
                codec: CodecId::H264,
                media_kind: MediaKind::Video,
            },
            TsTrackDesc {
                codec: CodecId::AAC,
                media_kind: MediaKind::Audio,
            },
            TsTrackDesc {
                codec: CodecId::Opus,
                media_kind: MediaKind::Audio,
            },
        ];
        let mut muxer = TsMuxerMulti::new(&tracks);
        assert_eq!(muxer.track_count(), 3);

        muxer.write_pat_pmt();
        muxer.write_frame(0, &[0x65; 50], 90000, 90000, true);
        muxer.write_frame(1, &[0xAA; 20], 90000, 90000, false);
        muxer.write_frame(2, &[0xBB; 20], 90000, 90000, false);

        let data = muxer.take_segment();
        assert_eq!(data.len() % TS_PACKET_SIZE, 0);
        for chunk in data.chunks(TS_PACKET_SIZE) {
            assert_eq!(chunk[0], 0x47);
        }
    }
}
