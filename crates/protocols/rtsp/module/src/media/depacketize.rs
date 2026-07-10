use super::*;

/// Builds `frame_from_rtp` output.
/// 构建 `frame_from_rtp` 输出.
pub(super) fn build_frame_from_rtp(
    track: &TrackInfo,
    packet: &RtpPacket,
    clock: &mut PublishTrackClock,
    h264_state: Option<&mut PublishH264Depacketizer>,
    h265_state: Option<&mut PublishH265Depacketizer>,
    av1_state: Option<&mut PublishAv1Depacketizer>,
) -> Option<BuiltFrameFromRtp> {
    let timebase = Timebase::new(1, track.clock_rate.max(1));
    let mut source_timestamp = packet.header.timestamp;
    let mut source_sequence = packet.header.sequence_number;
    let rtcp_mapping = if let (Some(lsr), Some(arrival_unix_micros)) =
        (clock.last_sr_lsr, clock.last_sr_unix_micros)
    {
        Some(RtpRtcpMapping {
            lsr,
            arrival_unix_micros,
        })
    } else {
        None
    };

    let mut built = match track.codec {
        CodecId::H264 => {
            let h264_state = h264_state?;
            let (payload, keyframe, access_unit_timestamp, access_unit_sequence) =
                depacketize_h264_access_unit(packet, h264_state)?;
            source_timestamp = access_unit_timestamp;
            source_sequence = access_unit_sequence;
            let mut frame = AVFrame::new(
                track.track_id,
                MediaKind::Video,
                CodecId::H264,
                FrameFormat::CanonicalH26x,
                i64::from(access_unit_timestamp),
                i64::from(access_unit_timestamp),
                timebase,
                payload,
            );
            if keyframe {
                frame.flags.insert(FrameFlags::KEY);
            }
            Some(BuiltFrameFromRtp {
                frame,
                discovered_audio_asc: None,
                discovered_av1_sequence_header: None,
                discovered_av1_codec_config: None,
                discovered_video_dimensions: None,
            })
        }
        CodecId::H265 | CodecId::H266 => {
            let h265_state = h265_state?;
            let (payload, keyframe, access_unit_timestamp, access_unit_sequence) =
                depacketize_h265_access_unit(track.codec, packet, h265_state)?;
            source_timestamp = access_unit_timestamp;
            source_sequence = access_unit_sequence;
            let mut frame = AVFrame::new(
                track.track_id,
                MediaKind::Video,
                track.codec,
                FrameFormat::CanonicalH26x,
                i64::from(access_unit_timestamp),
                i64::from(access_unit_timestamp),
                timebase,
                payload,
            );
            if keyframe {
                frame.flags.insert(FrameFlags::KEY);
            }
            Some(BuiltFrameFromRtp {
                frame,
                discovered_audio_asc: None,
                discovered_av1_sequence_header: None,
                discovered_av1_codec_config: None,
                discovered_video_dimensions: None,
            })
        }
        CodecId::AAC => {
            let payloads = depacketize_aac(
                &packet.payload,
                track.aac_rtp_packetization,
                track.aac_latm_config_in_band,
            )?;
            let payload = payloads.into_iter().next()?;
            let timestamp = source_timestamp.wrapping_add(payload.timestamp_offset);
            Some(BuiltFrameFromRtp {
                frame: AVFrame::new(
                    track.track_id,
                    MediaKind::Audio,
                    CodecId::AAC,
                    FrameFormat::AacRaw,
                    i64::from(timestamp),
                    i64::from(timestamp),
                    timebase,
                    payload.payload,
                ),
                discovered_audio_asc: payload.discovered_asc,
                discovered_av1_sequence_header: None,
                discovered_av1_codec_config: None,
                discovered_video_dimensions: None,
            })
        }
        CodecId::AV1 => {
            let av1_state = av1_state?;
            let (payload, keyframe, access_unit_timestamp, access_unit_sequence) =
                depacketize_av1_access_unit(packet, av1_state)?;
            let discovered_sequence_header = av1_sequence_header_obu_from_low_overhead(&payload);
            let discovered_dimensions = discovered_sequence_header
                .as_ref()
                .and_then(|sequence_header| av1_dimensions_from_sequence_header(sequence_header));
            let discovered_codec_config = discovered_sequence_header
                .as_ref()
                .and_then(|sequence_header| av1_codec_config_from_sequence_header(sequence_header));
            source_timestamp = access_unit_timestamp;
            source_sequence = access_unit_sequence;
            let mut frame = AVFrame::new(
                track.track_id,
                MediaKind::Video,
                CodecId::AV1,
                FrameFormat::CanonicalAv1Obu,
                i64::from(access_unit_timestamp),
                i64::from(access_unit_timestamp),
                timebase,
                payload,
            );
            frame
                .flags
                .insert(FrameFlags::START_OF_AU | FrameFlags::END_OF_AU);
            if keyframe {
                frame.flags.insert(FrameFlags::KEY);
            }
            Some(BuiltFrameFromRtp {
                frame,
                discovered_audio_asc: None,
                discovered_av1_sequence_header: discovered_sequence_header,
                discovered_av1_codec_config: discovered_codec_config,
                discovered_video_dimensions: discovered_dimensions,
            })
        }
        CodecId::VP9 => {
            build_vp9_frame_from_rtp(track, packet, clock, &mut PublishVp9Depacketizer::default())
        }
        CodecId::VP8 => {
            build_vp8_frame_from_rtp(track, packet, clock, &mut PublishVp8Depacketizer::default())
        }
        CodecId::Opus | CodecId::ADPCM | CodecId::G711A | CodecId::G711U | CodecId::MP3 => Some(
            build_passthrough_audio_frame(track, packet, i64::from(source_timestamp), timebase),
        ),
        _ => None,
    }?;
    let mut source_rtp = RtpTimestamp::new(source_timestamp, u64::from(source_timestamp));
    source_rtp.sequence_number = Some(source_sequence);
    source_rtp.rtcp_mapping = rtcp_mapping;
    built
        .frame
        .set_source_timestamp(SourceTimestamp::Rtp(source_rtp));
    Some(built)
}

/// Builds `vp9_frame_from_rtp` output.
/// 构建 `vp9_frame_from_rtp` 输出.
pub fn build_vp9_frame_from_rtp(
    track: &TrackInfo,
    packet: &RtpPacket,
    clock: &PublishTrackClock,
    state: &mut PublishVp9Depacketizer,
) -> Option<BuiltFrameFromRtp> {
    let timebase = Timebase::new(1, track.clock_rate.max(1));
    let (payload_offset, begin_of_frame) = vp9_payload_offset_and_begin(&packet.payload)?;
    let frame_payload = &packet.payload[payload_offset..];
    if frame_payload.is_empty() {
        reset_publish_vp9_depacketizer_state(state);
        return None;
    }

    if begin_of_frame
        || state.access_unit_timestamp != Some(packet.header.timestamp)
        || state.access_unit.is_empty()
    {
        reset_publish_vp9_depacketizer_state(state);
        state.access_unit_timestamp = Some(packet.header.timestamp);
        state.access_unit_keyframe = cheetah_codec::vp9_frame_is_keyframe(frame_payload);
    } else if state.access_unit_timestamp.is_none() {
        return None;
    }

    state.access_unit.extend_from_slice(frame_payload);
    state.access_unit_last_sequence = Some(packet.header.sequence_number);

    if !packet.header.marker {
        return None;
    }

    let payload = Bytes::copy_from_slice(&state.access_unit);
    let keyframe = state.access_unit_keyframe;
    let timestamp = state.access_unit_timestamp?;
    let sequence = state.access_unit_last_sequence;
    reset_publish_vp9_depacketizer_state(state);

    let mut frame = AVFrame::new(
        track.track_id,
        MediaKind::Video,
        CodecId::VP9,
        FrameFormat::CanonicalVp9Frame,
        i64::from(timestamp),
        i64::from(timestamp),
        timebase,
        payload,
    );
    frame
        .flags
        .insert(FrameFlags::START_OF_AU | FrameFlags::END_OF_AU);
    if keyframe {
        frame.flags.insert(FrameFlags::KEY);
    }

    let mut source_rtp = RtpTimestamp::new(timestamp, u64::from(timestamp));
    source_rtp.sequence_number = sequence;
    source_rtp.rtcp_mapping = if let (Some(lsr), Some(arrival_unix_micros)) =
        (clock.last_sr_lsr, clock.last_sr_unix_micros)
    {
        Some(RtpRtcpMapping {
            lsr,
            arrival_unix_micros,
        })
    } else {
        None
    };
    frame.set_source_timestamp(SourceTimestamp::Rtp(source_rtp));

    Some(BuiltFrameFromRtp {
        frame,
        discovered_audio_asc: None,
        discovered_av1_sequence_header: None,
        discovered_av1_codec_config: None,
        discovered_video_dimensions: None,
    })
}

/// Builds `vp8_frame_from_rtp` output.
/// 构建 `vp8_frame_from_rtp` 输出.
pub fn build_vp8_frame_from_rtp(
    track: &TrackInfo,
    packet: &RtpPacket,
    clock: &PublishTrackClock,
    state: &mut PublishVp8Depacketizer,
) -> Option<BuiltFrameFromRtp> {
    let timebase = Timebase::new(1, track.clock_rate.max(1));
    let (payload_offset, start_of_partition, partition_id) =
        vp8_payload_descriptor_info(&packet.payload)?;
    let frame_payload = &packet.payload[payload_offset..];
    if frame_payload.is_empty() {
        reset_publish_vp8_depacketizer_state(state);
        return None;
    }

    let start_of_frame = start_of_partition && partition_id == 0;
    if start_of_frame {
        reset_publish_vp8_depacketizer_state(state);
        state.access_unit_timestamp = Some(packet.header.timestamp);
        state.access_unit_keyframe = vp8_frame_is_keyframe(frame_payload);
    } else if state.access_unit_timestamp != Some(packet.header.timestamp)
        || state.access_unit.is_empty()
    {
        reset_publish_vp8_depacketizer_state(state);
        return None;
    }

    state.access_unit.extend_from_slice(frame_payload);
    state.access_unit_last_sequence = Some(packet.header.sequence_number);

    if !packet.header.marker {
        return None;
    }

    let payload = Bytes::copy_from_slice(&state.access_unit);
    let keyframe = state.access_unit_keyframe;
    let timestamp = state.access_unit_timestamp?;
    let sequence = state.access_unit_last_sequence;
    reset_publish_vp8_depacketizer_state(state);

    let mut frame = AVFrame::new(
        track.track_id,
        MediaKind::Video,
        CodecId::VP8,
        FrameFormat::CanonicalVp8Frame,
        i64::from(timestamp),
        i64::from(timestamp),
        timebase,
        payload,
    );
    frame
        .flags
        .insert(FrameFlags::START_OF_AU | FrameFlags::END_OF_AU);
    if keyframe {
        frame.flags.insert(FrameFlags::KEY);
    }

    let mut source_rtp = RtpTimestamp::new(timestamp, u64::from(timestamp));
    source_rtp.sequence_number = sequence;
    source_rtp.rtcp_mapping = if let (Some(lsr), Some(arrival_unix_micros)) =
        (clock.last_sr_lsr, clock.last_sr_unix_micros)
    {
        Some(RtpRtcpMapping {
            lsr,
            arrival_unix_micros,
        })
    } else {
        None
    };
    frame.set_source_timestamp(SourceTimestamp::Rtp(source_rtp));

    Some(BuiltFrameFromRtp {
        frame,
        discovered_audio_asc: None,
        discovered_av1_sequence_header: None,
        discovered_av1_codec_config: None,
        discovered_video_dimensions: None,
    })
}

/// Builds `frames_from_rtp` output.
/// 构建 `frames_from_rtp` 输出.
pub fn build_frames_from_rtp(
    track: &TrackInfo,
    packet: &RtpPacket,
    clock: &mut PublishTrackClock,
    h264_state: Option<&mut PublishH264Depacketizer>,
    h265_state: Option<&mut PublishH265Depacketizer>,
    av1_state: Option<&mut PublishAv1Depacketizer>,
) -> Vec<BuiltFrameFromRtp> {
    if track.codec != CodecId::AAC {
        return build_frame_from_rtp(track, packet, clock, h264_state, h265_state, av1_state)
            .into_iter()
            .collect();
    }

    let timebase = Timebase::new(1, track.clock_rate.max(1));
    let rtcp_mapping = if let (Some(lsr), Some(arrival_unix_micros)) =
        (clock.last_sr_lsr, clock.last_sr_unix_micros)
    {
        Some(RtpRtcpMapping {
            lsr,
            arrival_unix_micros,
        })
    } else {
        None
    };
    let Some(payloads) = depacketize_aac(
        &packet.payload,
        track.aac_rtp_packetization,
        track.aac_latm_config_in_band,
    ) else {
        return Vec::new();
    };

    payloads
        .into_iter()
        .map(|payload| {
            let timestamp = packet
                .header
                .timestamp
                .wrapping_add(payload.timestamp_offset);
            let mut frame = AVFrame::new(
                track.track_id,
                MediaKind::Audio,
                CodecId::AAC,
                FrameFormat::AacRaw,
                i64::from(timestamp),
                i64::from(timestamp),
                timebase,
                payload.payload,
            );
            let mut source_rtp = RtpTimestamp::new(timestamp, u64::from(timestamp));
            source_rtp.sequence_number = Some(packet.header.sequence_number);
            source_rtp.rtcp_mapping = rtcp_mapping;
            frame.set_source_timestamp(SourceTimestamp::Rtp(source_rtp));
            BuiltFrameFromRtp {
                frame,
                discovered_audio_asc: payload.discovered_asc,
                discovered_av1_sequence_header: None,
                discovered_av1_codec_config: None,
                discovered_video_dimensions: None,
            }
        })
        .collect()
}

fn build_passthrough_audio_frame(
    track: &TrackInfo,
    packet: &RtpPacket,
    pts: i64,
    timebase: Timebase,
) -> BuiltFrameFromRtp {
    let format = match track.codec {
        CodecId::Opus => FrameFormat::OpusPacket,
        CodecId::ADPCM => FrameFormat::AdpcmPacket,
        CodecId::G711A | CodecId::G711U => FrameFormat::G711Packet,
        CodecId::MP3 => FrameFormat::Mp3Frame,
        _ => FrameFormat::DataPacket,
    };
    let mut frame = AVFrame::new(
        track.track_id,
        MediaKind::Audio,
        track.codec,
        format,
        pts,
        pts,
        timebase,
        packet.payload.clone(),
    );
    frame.flags.insert(FrameFlags::START_OF_AU);
    if packet.header.marker {
        frame.flags.insert(FrameFlags::END_OF_AU);
    }
    BuiltFrameFromRtp {
        frame,
        discovered_audio_asc: None,
        discovered_av1_sequence_header: None,
        discovered_av1_codec_config: None,
        discovered_video_dimensions: None,
    }
}

/// `av1_rtp_payload_is_keyframe` function.
/// `av1_rtp_payload_is_keyframe` 函数.
#[cfg(test)]
pub(super) fn av1_rtp_payload_is_keyframe(payload: &[u8]) -> bool {
    if payload.len() < 2 {
        return false;
    }
    // AV1 RTP aggregation header: Z Y W W N - - -
    // Parse AV1 frame/frame-header OBUs and derive keyframe from frame_type bits.
    // Do not use N-bit (start of coded video sequence) as keyframe proxy.
    let aggregation = payload[0];
    let z = (aggregation & 0x80) != 0;
    if z {
        return false;
    }
    let w = ((aggregation >> 4) & 0x03) as usize;
    let mut cursor = &payload[1..];

    if w == 0 {
        while !cursor.is_empty() {
            let Some((obu_len, leb_len)) = av1_read_leb128(cursor) else {
                return false;
            };
            cursor = &cursor[leb_len..];
            if obu_len > cursor.len() {
                return false;
            }
            let obu = &cursor[..obu_len];
            if let Some(is_key) = av1_obu_is_keyframe(obu) {
                return is_key;
            }
            cursor = &cursor[obu_len..];
        }
        return false;
    }

    for index in 0..w {
        let obu = if index + 1 == w {
            let last = cursor;
            cursor = &[];
            last
        } else {
            let Some((obu_len, leb_len)) = av1_read_leb128(cursor) else {
                return false;
            };
            cursor = &cursor[leb_len..];
            if obu_len > cursor.len() {
                return false;
            }
            let obu = &cursor[..obu_len];
            cursor = &cursor[obu_len..];
            obu
        };
        if let Some(is_key) = av1_obu_is_keyframe(obu) {
            return is_key;
        }
    }
    false
}

/// `av1_obu_is_keyframe` function.
/// `av1_obu_is_keyframe` 函数.
pub(super) fn av1_obu_is_keyframe(obu: &[u8]) -> Option<bool> {
    let obu_header = *obu.first()?;
    let obu_type = (obu_header >> 3) & 0x0f;
    let has_extension = (obu_header & 0x04) != 0;
    let mut offset = 1usize;
    if has_extension {
        offset = offset.checked_add(1)?;
    }
    let payload = obu.get(offset..)?;
    match obu_type {
        3 | 6 | 7 => av1_frame_header_is_keyframe(payload),
        _ => None,
    }
}

fn av1_frame_header_is_keyframe(payload: &[u8]) -> Option<bool> {
    let mut bits = BitReader::new(payload);
    let show_existing_frame = bits.read_bit()?;
    if show_existing_frame != 0 {
        return Some(false);
    }
    let frame_type = bits.read_bits(2)? as u8;
    Some(frame_type == 0)
}

/// `av1_read_leb128` function.
/// `av1_read_leb128` 函数.
pub(super) fn av1_read_leb128(data: &[u8]) -> Option<(usize, usize)> {
    let mut value: usize = 0;
    let mut shift: u32 = 0;
    for (index, byte) in data.iter().copied().take(8).enumerate() {
        let part = usize::from(byte & 0x7f);
        value |= part.checked_shl(shift)?;
        if (byte & 0x80) == 0 {
            return Some((value, index + 1));
        }
        shift = shift.checked_add(7)?;
    }
    None
}

/// `vp8_rtp_payload_is_keyframe` function.
/// `vp8_rtp_payload_is_keyframe` 函数.
#[cfg(test)]
pub(super) fn vp8_rtp_payload_is_keyframe(payload: &[u8]) -> bool {
    let Some((offset, start, partition_id)) = vp8_payload_descriptor_info(payload) else {
        return false;
    };
    if !start || partition_id != 0 || payload.len() <= offset {
        return false;
    }
    vp8_frame_is_keyframe(&payload[offset..])
}

fn vp8_payload_descriptor_info(payload: &[u8]) -> Option<(usize, bool, u8)> {
    if payload.is_empty() {
        return None;
    }
    let mut offset = 1usize;
    let descriptor = payload[0];
    let has_extension = (descriptor & 0x80) != 0;
    let start = (descriptor & 0x10) != 0;
    let partition_id = descriptor & 0x0f;
    if has_extension {
        let ext = *payload.get(offset)?;
        offset += 1;
        if (ext & 0x80) != 0 {
            let pic = *payload.get(offset)?;
            offset += 1;
            if (pic & 0x80) != 0 {
                offset += 1;
            }
        }
        if (ext & 0x40) != 0 {
            offset += 1;
        }
        if (ext & 0x20) != 0 || (ext & 0x10) != 0 {
            offset += 1;
        }
    }
    if payload.len() <= offset {
        return None;
    }
    Some((offset, start, partition_id))
}

/// `vp9_rtp_payload_is_keyframe` function.
/// `vp9_rtp_payload_is_keyframe` 函数.
#[cfg(test)]
pub(super) fn vp9_rtp_payload_is_keyframe(payload: &[u8]) -> bool {
    if payload.is_empty() {
        return false;
    }
    let descriptor = payload[0];
    let begin_of_frame = (descriptor & 0x08) != 0;
    if !begin_of_frame {
        return false;
    }
    let Some(offset) = vp9_payload_offset(payload) else {
        return false;
    };
    if payload.len() <= offset {
        return false;
    }
    cheetah_codec::vp9_frame_is_keyframe(&payload[offset..])
}

fn vp9_payload_offset(payload: &[u8]) -> Option<usize> {
    if payload.is_empty() {
        return None;
    }
    let descriptor = payload[0];
    let has_picture_id = (descriptor & 0x80) != 0;
    let is_inter_picture_predicted = (descriptor & 0x40) != 0;
    let has_layer_indices = (descriptor & 0x20) != 0;
    let flexible_mode = (descriptor & 0x10) != 0;
    let has_scalability_structure = (descriptor & 0x02) != 0;

    let mut offset = 1usize;
    if has_picture_id {
        let pic = *payload.get(offset)?;
        offset += 1;
        if (pic & 0x80) != 0 {
            offset += 1;
        }
    }
    if has_layer_indices {
        offset += 1;
        if !flexible_mode {
            offset += 1;
        }
    }
    if flexible_mode && is_inter_picture_predicted {
        loop {
            let reference = *payload.get(offset)?;
            offset += 1;
            if (reference & 0x01) == 0 {
                break;
            }
        }
    }
    if has_scalability_structure {
        let ss = *payload.get(offset)?;
        offset += 1;
        let spatial_layers = usize::from((ss >> 5) & 0x07) + 1;
        let has_resolution = (ss & 0x10) != 0;
        let has_group = (ss & 0x08) != 0;
        if has_resolution {
            offset = offset.checked_add(spatial_layers.checked_mul(4)?)?;
        }
        if has_group {
            let group_count = usize::from(ss & 0x07);
            for _ in 0..group_count {
                let group = *payload.get(offset)?;
                offset += 1;
                let refs = usize::from((group >> 2) & 0x03);
                offset = offset.checked_add(refs)?;
            }
        }
    }
    if payload.len() <= offset {
        return None;
    }
    Some(offset)
}

fn vp9_payload_offset_and_begin(payload: &[u8]) -> Option<(usize, bool)> {
    let descriptor = *payload.first()?;
    let begin_of_frame = (descriptor & 0x08) != 0;
    vp9_payload_offset(payload).map(|offset| (offset, begin_of_frame))
}
