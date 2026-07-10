use super::*;

/// `packetize_frame_to_rtp_with_timestamp` function.
/// `packetize_frame_to_rtp_with_timestamp` 函数.
pub fn packetize_frame_to_rtp_with_timestamp(
    frame: &AVFrame,
    track: &TrackInfo,
    payload_type: u8,
    seq: &mut u16,
    ssrc: u32,
    mtu: usize,
    timestamp: u32,
) -> Vec<RtpPacket> {
    match track.codec {
        CodecId::H264 => packetize_h264(
            frame.payload.as_ref(),
            payload_type,
            seq,
            timestamp,
            ssrc,
            mtu,
        ),
        CodecId::H265 => packetize_h265(
            frame.payload.as_ref(),
            payload_type,
            seq,
            timestamp,
            ssrc,
            mtu,
        ),
        CodecId::AAC => packetize_aac(
            frame.payload.as_ref(),
            payload_type,
            seq,
            timestamp,
            ssrc,
            mtu,
        ),
        CodecId::AV1 if frame.format == FrameFormat::CanonicalAv1Obu => {
            packetize_av1(frame, track, payload_type, seq, timestamp, ssrc, mtu)
        }
        CodecId::H266
        | CodecId::AV1
        | CodecId::VP8
        | CodecId::VP9
        | CodecId::Opus
        | CodecId::ADPCM
        | CodecId::G711A
        | CodecId::G711U
        | CodecId::MP3 => {
            let has_explicit_boundary = frame
                .flags
                .intersects(FrameFlags::START_OF_AU | FrameFlags::END_OF_AU);
            let marker = if has_explicit_boundary {
                frame.flags.contains(FrameFlags::END_OF_AU)
            } else {
                true
            };
            packetize_passthrough(
                frame.payload.as_ref(),
                payload_type,
                seq,
                timestamp,
                ssrc,
                mtu,
                marker,
            )
        }
        _ => Vec::new(),
    }
}
