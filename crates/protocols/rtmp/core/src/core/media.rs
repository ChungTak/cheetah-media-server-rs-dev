use alloc::string::ToString;
use alloc::vec::Vec;

use bytes::Bytes;

use super::{
    CoreOutput, PendingPublishMedia, RtmpCore, RtmpEvent, RtmpMediaType,
    MAX_PENDING_PUBLISH_MEDIA_BYTES, MAX_PENDING_PUBLISH_MEDIA_EVENTS,
};

impl RtmpCore {
    /// Routes media payload to either the active publish or the pending publish buffer.
    /// 将媒体负载路由到活跃发布或待发布缓冲区。
    pub(super) fn handle_media_input(
        &mut self,
        stream_id: u32,
        timestamp_ms: u32,
        media_type: RtmpMediaType,
        payload: Bytes,
        out: &mut Vec<CoreOutput>,
    ) {
        if self.active_publish == Some(stream_id) {
            out.push(CoreOutput::Event(RtmpEvent::MediaData {
                stream_id,
                timestamp_ms,
                media_type,
                payload,
            }));
            return;
        }
        if self.pending_publish == Some(stream_id) {
            let is_sequence_header = is_sequence_header(&media_type, &payload);
            self.push_pending_publish_media(
                stream_id,
                timestamp_ms,
                media_type,
                payload,
                is_sequence_header,
            );
            return;
        }
        out.push(CoreOutput::Event(RtmpEvent::MessageIgnored {
            name: "MediaData".to_string(),
            detail: format!("media from unauthenticated stream_id={stream_id} dropped"),
        }));
    }

    /// Buffers media received before the publish is accepted, dropping non-sequence headers when over limit.
    /// 在发布被接受前缓冲收到的媒体，超限时丢弃非序列头。
    pub(super) fn push_pending_publish_media(
        &mut self,
        stream_id: u32,
        timestamp_ms: u32,
        media_type: RtmpMediaType,
        payload: Bytes,
        is_sequence_header: bool,
    ) {
        let payload_len = payload.len();
        if payload_len > MAX_PENDING_PUBLISH_MEDIA_BYTES {
            return;
        }

        while self.pending_media.len() >= MAX_PENDING_PUBLISH_MEDIA_EVENTS
            || self.pending_media_bytes.saturating_add(payload_len)
                > MAX_PENDING_PUBLISH_MEDIA_BYTES
        {
            let removable_idx = self
                .pending_media
                .iter()
                .rposition(|m| !m.is_sequence_header);
            let Some(idx) = removable_idx else {
                break;
            };
            let Some(removed) = self.pending_media.remove(idx) else {
                break;
            };
            self.pending_media_bytes = self
                .pending_media_bytes
                .saturating_sub(removed.payload.len());
        }

        self.pending_media.push_back(PendingPublishMedia {
            stream_id,
            timestamp_ms,
            media_type,
            payload,
            is_sequence_header,
        });
        self.pending_media_bytes = self.pending_media_bytes.saturating_add(payload_len);
    }

    /// Emits all buffered pending publish media as events and clears the pending state.
    /// 发出所有缓冲的待发布媒体事件并清空待发布状态。
    pub(super) fn flush_pending_publish_media(
        &mut self,
        stream_id: u32,
        out: &mut Vec<CoreOutput>,
    ) {
        if self.pending_publish != Some(stream_id) {
            return;
        }
        while let Some(media) = self.pending_media.pop_front() {
            self.pending_media_bytes = self.pending_media_bytes.saturating_sub(media.payload.len());
            out.push(CoreOutput::Event(RtmpEvent::MediaData {
                stream_id: media.stream_id,
                timestamp_ms: media.timestamp_ms,
                media_type: media.media_type,
                payload: media.payload,
            }));
        }
        self.pending_publish = None;
    }

    /// Clears the pending publish state and buffered media.
    /// 清除待发布状态与缓冲的媒体。
    pub(super) fn clear_pending_publish(&mut self) {
        self.pending_publish = None;
        self.pending_media.clear();
        self.pending_media_bytes = 0;
    }
}

const EX_VIDEO_FLAG: u8 = 0x80;
const EX_VIDEO_PACKET_TYPE_SEQUENCE_START: u8 = 0;

/// Detects whether a media payload is an H.264/H.265/AV1/H.266 or AAC sequence header.
/// 检测媒体负载是否为 H.264/H.265/AV1/H.266 或 AAC 的序列头。
fn is_sequence_header(media_type: &RtmpMediaType, payload: &[u8]) -> bool {
    let first = match payload.first() {
        Some(&b) => b,
        None => return false,
    };
    match media_type {
        RtmpMediaType::Video => {
            if first & EX_VIDEO_FLAG != 0 {
                return payload.get(5) == Some(&EX_VIDEO_PACKET_TYPE_SEQUENCE_START);
            }
            let codec_id = first & 0x0F;
            match codec_id {
                cheetah_codec::RTMP_VIDEO_CODEC_ID_H264
                | cheetah_codec::RTMP_VIDEO_CODEC_ID_H265
                | cheetah_codec::RTMP_VIDEO_CODEC_ID_AV1
                | cheetah_codec::RTMP_VIDEO_CODEC_ID_H266 => payload.get(1) == Some(&0),
                _ => false,
            }
        }
        RtmpMediaType::Audio => {
            let sound_format = first >> 4;
            if sound_format == 10 {
                payload.get(1) == Some(&0)
            } else {
                false
            }
        }
        RtmpMediaType::Data => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h264_seq_header() -> (RtmpMediaType, Vec<u8>) {
        (
            RtmpMediaType::Video,
            vec![0x17, 0x00, 0x00, 0x00, 0x00, 0x01],
        )
    }

    fn h264_nal_unit() -> (RtmpMediaType, Vec<u8>) {
        (
            RtmpMediaType::Video,
            vec![0x17, 0x01, 0x00, 0x00, 0x00, 0x01],
        )
    }

    fn h265_seq_header() -> (RtmpMediaType, Vec<u8>) {
        (
            RtmpMediaType::Video,
            vec![0x1C, 0x00, 0x00, 0x00, 0x00, 0x01],
        )
    }

    fn av1_seq_header() -> (RtmpMediaType, Vec<u8>) {
        (
            RtmpMediaType::Video,
            vec![0x0D, 0x00, 0x00, 0x00, 0x00, 0x01],
        )
    }

    fn h266_seq_header() -> (RtmpMediaType, Vec<u8>) {
        (
            RtmpMediaType::Video,
            vec![0x1E, 0x00, 0x00, 0x00, 0x00, 0x01],
        )
    }

    fn enhanced_av1_seq_header() -> (RtmpMediaType, Vec<u8>) {
        (
            RtmpMediaType::Video,
            vec![0x81, b'a', b'v', b'0', b'1', 0x00, 0x01],
        )
    }

    fn enhanced_vp9_seq_header() -> (RtmpMediaType, Vec<u8>) {
        (
            RtmpMediaType::Video,
            vec![0x81, b'v', b'p', b'0', b'9', 0x00, 0x01],
        )
    }

    fn enhanced_h265_coded_frames() -> (RtmpMediaType, Vec<u8>) {
        (
            RtmpMediaType::Video,
            vec![0x82, b'h', b'v', b'c', b'1', 0x02, 0x00, 0x00, 0x00],
        )
    }

    fn aac_seq_header() -> (RtmpMediaType, Vec<u8>) {
        (RtmpMediaType::Audio, vec![0xAF, 0x00, 0x12, 0x10])
    }

    fn aac_raw() -> (RtmpMediaType, Vec<u8>) {
        (RtmpMediaType::Audio, vec![0xAF, 0x01, 0x21, 0x00])
    }

    #[test]
    fn legacy_h264_sequence_header() {
        let (mt, p) = h264_seq_header();
        assert!(is_sequence_header(&mt, &p));
    }

    #[test]
    fn legacy_h264_nal_unit_not_sequence_header() {
        let (mt, p) = h264_nal_unit();
        assert!(!is_sequence_header(&mt, &p));
    }

    #[test]
    fn legacy_h265_sequence_header() {
        let (mt, p) = h265_seq_header();
        assert!(is_sequence_header(&mt, &p));
    }

    #[test]
    fn legacy_av1_sequence_header() {
        let (mt, p) = av1_seq_header();
        assert!(is_sequence_header(&mt, &p));
    }

    #[test]
    fn legacy_h266_sequence_header() {
        let (mt, p) = h266_seq_header();
        assert!(is_sequence_header(&mt, &p));
    }

    #[test]
    fn enhanced_av1_sequence_start() {
        let (mt, p) = enhanced_av1_seq_header();
        assert!(is_sequence_header(&mt, &p));
    }

    #[test]
    fn enhanced_vp9_sequence_start() {
        let (mt, p) = enhanced_vp9_seq_header();
        assert!(is_sequence_header(&mt, &p));
    }

    #[test]
    fn enhanced_h265_coded_frames_not_sequence_header() {
        let (mt, p) = enhanced_h265_coded_frames();
        assert!(!is_sequence_header(&mt, &p));
    }

    #[test]
    fn aac_sequence_header() {
        let (mt, p) = aac_seq_header();
        assert!(is_sequence_header(&mt, &p));
    }

    #[test]
    fn aac_raw_not_sequence_header() {
        let (mt, p) = aac_raw();
        assert!(!is_sequence_header(&mt, &p));
    }

    #[test]
    fn empty_payload_not_sequence_header() {
        assert!(!is_sequence_header(&RtmpMediaType::Video, &[]));
    }

    #[test]
    fn data_never_sequence_header() {
        assert!(!is_sequence_header(&RtmpMediaType::Data, &[0x17, 0x00]));
    }
}
