//! FLV egress mapping for RTMP.
//!
//! The FLV frame↔payload encapsulation lives in `cheetah-codec`
//! (`cheetah_codec::flv_egress`) and is shared with HTTP-FLV. This module
//! re-exports it to preserve the RTMP-core public API surface.

pub use cheetah_codec::{
    build_h265_config, build_h266_config, build_metadata, build_track_bootstrap_payloads,
    build_video_config_payload, frame_dts_to_rtmp_timestamp_ms, map_frame_to_rtmp_flv_payload,
    mute_aac_config_payload, mute_aac_frame_payload, rtmp_playback_codec_supported,
    track_list_has_audio, use_enhanced_video_mode, RtmpFlvPayload, RtmpFlvPayloadKind,
    RtmpFlvPlayMode,
};

pub use crate::media::{
    decode_audio_frame, decode_video_frame, encode_audio_frame, encode_video_frame,
};

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use cheetah_codec::{
        rtmp_fourcc_from_codec, AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase,
        TrackId, TrackInfo,
    };

    use super::*;
    use crate::{decode_all, Amf0Value};

    #[test]
    fn av1_enhanced_video_frame_does_not_insert_composition_time_prefix() {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::AV1,
            FrameFormat::CanonicalAv1Obu,
            3_000,
            0,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x0a, 0x0e, 0x4a]),
        );
        frame.flags.insert(FrameFlags::KEY);

        let payload = map_frame_to_rtmp_flv_payload(&frame, RtmpFlvPlayMode::Normal, &[])
            .expect("av1 payload");
        let fourcc = rtmp_fourcc_from_codec(CodecId::AV1)
            .expect("av1 fourcc")
            .to_be_bytes();

        assert_eq!(payload.kind, RtmpFlvPayloadKind::Video);
        assert_eq!(payload.payload[0], 0x91);
        assert_eq!(&payload.payload[1..5], &fourcc);
        assert_eq!(
            &payload.payload[5..],
            &[0x0a, 0x0e, 0x4a],
            "AV1 enhanced RTMP coded frame must start with AV1 OBU bytes, not CTS"
        );
    }

    #[test]
    fn metadata_uses_enhanced_video_fourcc_for_av1() {
        let track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::AV1, 90_000);

        let metadata = build_metadata(&[track]);
        let values = decode_all(&metadata).expect("decode metadata");
        let Some(Amf0Value::EcmaArray { entries }) = values.get(1) else {
            panic!("metadata must contain an ECMA array");
        };

        let video_codec_id = entries
            .iter()
            .find(|entry| entry.key == "videocodecid")
            .and_then(|entry| entry.value.as_f64())
            .expect("videocodecid");
        let av1_fourcc = rtmp_fourcc_from_codec(CodecId::AV1).expect("av1 fourcc") as f64;
        assert_eq!(video_codec_id, av1_fourcc);
    }
}
