#![no_std]

#[macro_use]
extern crate alloc;

pub(crate) mod prelude {
    pub use alloc::borrow::ToOwned;
    pub use alloc::string::{String, ToString};
    pub use alloc::vec::Vec;
}

pub mod amf;
pub mod amf0;
pub mod amf3;
pub mod bytes;
pub mod chunk;
pub mod command;
pub mod core;
pub mod error;
pub mod flv;
pub mod flv_ingest;
pub mod handshake;
#[cfg(feature = "complex-handshake")]
pub mod handshake_complex;
pub mod media;
pub mod message;
pub mod timestamp;
pub mod url;
pub mod user_control;

pub use amf::{AmfValue, AmfValueRef, AmfVersion, Pair};
pub use amf0::{decode_all, encode_all, Amf0Error, Amf0Value};
pub use amf3::Amf3Value;
pub use bytes::{Buf, BytesReader, BytesWriter};
pub use chunk::{
    MessageHeaderFormat, RtmpChunk, RtmpChunkDecoder, RtmpChunkEncoder, RtmpChunkSize,
    RtmpChunkStreamId,
};
pub use command::{
    RtmpCommand, RtmpConnectCommand, RtmpCreateStreamCommand, RtmpDeleteStreamCommand,
    RtmpGetStreamLengthCommand, RtmpOnStatusCommand, RtmpPlayCommand, RtmpPublishCommand,
    RtmpResultCommand, TransactionId,
};
pub use core::{
    CoreInput, CoreOutput, RtmpClientState, RtmpCore, RtmpCoreCommand, RtmpCoreError, RtmpEvent,
    RtmpMediaType, TimerId,
};
pub use error::{Error, ErrorKind};
pub use flv::{
    build_h265_config, build_h266_config, build_metadata, build_track_bootstrap_payloads,
    build_video_config_payload, frame_dts_to_rtmp_timestamp_ms, map_frame_to_rtmp_flv_payload,
    mute_aac_config_payload, mute_aac_frame_payload, rtmp_playback_codec_supported,
    track_list_has_audio, use_enhanced_video_mode, RtmpFlvPayload, RtmpFlvPayloadKind,
    RtmpFlvPlayMode,
};
pub use flv_ingest::{
    apply_flv_metadata_to_tracks, apply_flv_video_config, attach_raw_rtmp_audio_payload,
    attach_raw_rtmp_video_payload, length_prefixed_to_annexb_with_size,
    maybe_reset_rtmp_timestamp_normalizer, parse_flv_avcc_parameter_sets,
    parse_flv_hvcc_parameter_sets, source_timestamp_from_rtmp_ms, update_timestamp_repair_counter,
    RTMP_AUDIO_RAW_SIDEDATA_MAGIC, RTMP_VIDEO_RAW_SIDEDATA_MAGIC,
};
pub use handshake::{ClientHandshakeMode, RtmpClientHandshake, RtmpServerHandshake};
pub use media::{
    decode_audio_frame, decode_video_frame, encode_audio_frame, encode_video_frame,
    parse_video_ingress_header, parse_video_ingress_header_with_mode, parse_video_multi_track,
    AudioFormat, AudioFrame, AudioSampleRate, AvcPacketType, AvcSequenceHeader, MediaFrame,
    MultiTrackEntry, MultiTrackType, RtmpTimestamp, RtmpTimestampDelta, VideoCodec, VideoFrame,
    VideoFrameType, VideoIngressHeader, VideoMultiTrackHeader,
};
pub use message::{
    RtmpMessage, RtmpMessageDecoder, RtmpMessageEncoder, RtmpMessageHeader, RtmpMessageStreamId,
    RtmpMessageType, SetPeerBandwidthLimitType,
};
pub use url::RtmpUrl;
pub use user_control::RtmpUserControlEvent;

#[cfg(test)]
mod tests_no_std {
    use super::{CoreInput, RtmpCore};

    #[test]
    fn timeout_input_smoke() {
        let mut core = RtmpCore::new();
        let out = core
            .handle_input(CoreInput::Timeout { id: 1 })
            .expect("timeout input should be accepted");
        assert!(out.is_empty());
    }
}
