use super::*;
use crate::media::TransportInterleaved;
/// `header_value` function.
/// `header_value` 函数.
pub(super) fn header_value<'a>(req: &'a RtspRequest, name: &str) -> Option<&'a str> {
    req.header_value(name)
}

/// `default_payload_type` function.
/// `default_payload_type` 函数.
pub(super) fn default_payload_type(codec: cheetah_codec::CodecId) -> u8 {
    match codec {
        cheetah_codec::CodecId::G711U => 0,
        cheetah_codec::CodecId::G711A => 8,
        cheetah_codec::CodecId::MP3 => 14,
        _ => 96,
    }
}

/// `format_rtp_ssrc` function.
/// `format_rtp_ssrc` 函数.
pub(super) fn format_rtp_ssrc(ssrc: u32) -> String {
    format!("{ssrc:08X}")
}

/// `wildcard_bind_addr` function.
/// `wildcard_bind_addr` 函数.
pub(super) fn wildcard_bind_addr(peer: SocketAddr) -> SocketAddr {
    if peer.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0u16; 8], 0))
    }
}

/// `publish_track_is_already_setup` function.
/// `publish_track_is_already_setup` 函数.
pub(super) fn publish_track_is_already_setup(publish: &PublishSession, track_id: TrackId) -> bool {
    publish.udp_tracks.contains_key(&track_id)
        || publish
            .track_channels
            .values()
            .any(|configured_track_id| *configured_track_id == track_id)
        || publish
            .rtcp_channels
            .values()
            .any(|configured_track_id| *configured_track_id == track_id)
}

/// `interleaved_channels_in_use` function.
/// `interleaved_channels_in_use` 函数.
pub(super) fn interleaved_channels_in_use(
    track_channels: &HashMap<u8, TrackId>,
    rtcp_channels: &HashMap<u8, TrackId>,
    rtp_channel: u8,
    rtcp_channel: u8,
) -> bool {
    track_channels.contains_key(&rtp_channel)
        || track_channels.contains_key(&rtcp_channel)
        || rtcp_channels.contains_key(&rtp_channel)
        || rtcp_channels.contains_key(&rtcp_channel)
}

/// `play_interleaved_channels_conflict` function.
/// `play_interleaved_channels_conflict` 函数.
pub(super) fn play_interleaved_channels_conflict(
    play_tracks: &HashMap<TrackId, PlayTrackState>,
    target_track_id: TrackId,
    rtp_channel: u8,
    rtcp_channel: u8,
) -> bool {
    play_tracks.iter().any(|(track_id, state)| {
        if *track_id == target_track_id {
            return false;
        }
        match &state.transport {
            PlayTransport::TcpInterleaved {
                rtp_channel: existing_rtp,
                rtcp_channel: existing_rtcp,
            } => {
                *existing_rtp == rtp_channel
                    || *existing_rtp == rtcp_channel
                    || *existing_rtcp == rtp_channel
                    || *existing_rtcp == rtcp_channel
            }
            PlayTransport::UdpUnicast { .. } | PlayTransport::UdpMulticast { .. } => false,
        }
    })
}

/// `next_publish_interleaved_channels` function.
/// `next_publish_interleaved_channels` 函数.
pub(super) fn next_publish_interleaved_channels(
    track_channels: &HashMap<u8, TrackId>,
    rtcp_channels: &HashMap<u8, TrackId>,
) -> Option<TransportInterleaved> {
    for rtp_channel in (0u8..=254u8).step_by(2) {
        let rtcp_channel = rtp_channel.saturating_add(1);
        if !interleaved_channels_in_use(track_channels, rtcp_channels, rtp_channel, rtcp_channel) {
            return Some(TransportInterleaved {
                rtp_channel,
                rtcp_channel,
            });
        }
    }
    None
}

/// `next_play_interleaved_channels` function.
/// `next_play_interleaved_channels` 函数.
pub(super) fn next_play_interleaved_channels(
    play_tracks: &HashMap<TrackId, PlayTrackState>,
    target_track_id: TrackId,
) -> Option<TransportInterleaved> {
    for rtp_channel in (0u8..=254u8).step_by(2) {
        let rtcp_channel = rtp_channel.saturating_add(1);
        if !play_interleaved_channels_conflict(
            play_tracks,
            target_track_id,
            rtp_channel,
            rtcp_channel,
        ) {
            return Some(TransportInterleaved {
                rtp_channel,
                rtcp_channel,
            });
        }
    }
    None
}

/// `publish_configured_track_count` function.
/// `publish_configured_track_count` 函数.
pub(super) fn publish_configured_track_count(publish: &PublishSession) -> usize {
    let mut track_ids = HashSet::new();
    for track_id in publish.udp_tracks.keys().copied() {
        track_ids.insert(track_id);
    }
    for track_id in publish.track_channels.values().copied() {
        track_ids.insert(track_id);
    }
    for track_id in publish.rtcp_channels.values().copied() {
        track_ids.insert(track_id);
    }
    track_ids.len()
}

/// `runtime_unix_time_micros` function.
/// `runtime_unix_time_micros` 函数.
pub(super) fn runtime_unix_time_micros(runtime_api: &Arc<dyn RuntimeApi>) -> u64 {
    runtime_api.now().as_micros()
}

/// `normalize_play_range_header` function.
/// `normalize_play_range_header` 函数.
pub(super) fn normalize_play_range_header(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() || value.contains('\r') || value.contains('\n') {
        return None;
    }
    if !value
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("npt="))
    {
        return None;
    }

    let spec = &value[4..];
    let (start_raw, end_raw) = spec.split_once('-')?;
    let start = start_raw.trim();
    let end = end_raw.trim();
    if start.is_empty() && end.is_empty() {
        return None;
    }
    if end.eq_ignore_ascii_case("now") {
        return None;
    }
    if start.eq_ignore_ascii_case("now") && !end.is_empty() {
        return None;
    }
    if !start.is_empty() && !is_valid_npt_range_part(start) {
        return None;
    }
    if !end.is_empty() && !is_valid_npt_range_part(end) {
        return None;
    }
    if let (Some(start_seconds), Some(end_seconds)) =
        (parse_npt_seconds(start), parse_npt_seconds(end))
    {
        if end_seconds + 1e-9f64 < start_seconds {
            return None;
        }
    }
    Some(format!("npt={start}-{end}"))
}

/// Parses `play_range_header` from input.
/// 解析 `play_range_header` 来自 输入.
pub(super) fn parse_play_range_header(
    raw: Option<&str>,
) -> Result<Option<String>, (u16, &'static str, &'static [u8])> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    normalize_play_range_header(raw).map(Some).ok_or((
        457,
        "Invalid Range",
        b"invalid Range header",
    ))
}

/// Parses `request_range_scale_headers` from input.
/// 解析 `request_range_scale_headers` 来自 输入.
pub(super) fn parse_request_range_scale_headers(
    req: &RtspRequest,
) -> Result<Option<String>, RtspErrorResponse> {
    validate_play_scale_header(header_value(req, "scale"))?;
    parse_play_range_header(header_value(req, "range"))
}

/// `validate_play_scale_header` function.
/// `validate_play_scale_header` 函数.
pub(super) fn validate_play_scale_header(
    raw: Option<&str>,
) -> Result<(), (u16, &'static str, &'static [u8])> {
    let Some(raw) = raw else {
        return Ok(());
    };
    let value = raw.trim();
    if value.is_empty() {
        return Err((400, "Bad Request", b"invalid Scale header"));
    }
    let parsed = match value.parse::<f64>() {
        Ok(parsed) => parsed,
        Err(_) => return Err((400, "Bad Request", b"invalid Scale header")),
    };
    if !parsed.is_finite() {
        return Err((400, "Bad Request", b"invalid Scale header"));
    }
    if (parsed - 1.0f64).abs() > 1e-6f64 {
        return Err((406, "Not Acceptable", b"only Scale: 1.0 is supported"));
    }
    Ok(())
}

/// Returns `true` if `valid_npt_range_part` is true.
/// 返回 `真` 如果 `valid_npt_range_part` is 真.
pub(super) fn is_valid_npt_range_part(value: &str) -> bool {
    value.eq_ignore_ascii_case("now")
        || value
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b':')
}

/// Parses `npt_seconds` from input.
/// 解析 `npt_seconds` 来自 输入.
pub(super) fn parse_npt_seconds(value: &str) -> Option<f64> {
    if value.is_empty() || value.eq_ignore_ascii_case("now") {
        return None;
    }
    if value.contains(':') {
        let mut parts = value.split(':');
        let hour = parts.next()?.trim().parse::<u64>().ok()? as f64;
        let minute_raw = parts.next()?.trim().parse::<u64>().ok()?;
        let second_raw = parts.next()?.trim().parse::<f64>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        if minute_raw >= 60 || !(0.0f64..60.0f64).contains(&second_raw) {
            return None;
        }
        let minute = minute_raw as f64;
        let second = second_raw;
        return Some(hour * 3600.0f64 + minute * 60.0f64 + second);
    }
    value.trim().parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::PlayTransport;
    use cheetah_codec::{CodecId, TrackId};

    #[test]
    fn default_payload_type_aligns_with_sdp_defaults() {
        assert_eq!(default_payload_type(CodecId::AAC), 96);
        assert_eq!(default_payload_type(CodecId::H264), 96);
        assert_eq!(default_payload_type(CodecId::G711U), 0);
        assert_eq!(default_payload_type(CodecId::G711A), 8);
        assert_eq!(default_payload_type(CodecId::MP3), 14);
    }

    #[test]
    fn next_publish_interleaved_channels_allocates_first_free_pair() {
        let mut track_channels = HashMap::new();
        let mut rtcp_channels = HashMap::new();
        track_channels.insert(0, TrackId(1));
        rtcp_channels.insert(1, TrackId(1));
        track_channels.insert(2, TrackId(2));
        rtcp_channels.insert(3, TrackId(2));
        let next = next_publish_interleaved_channels(&track_channels, &rtcp_channels)
            .expect("next publish interleaved channels");
        assert_eq!(next.rtp_channel, 4);
        assert_eq!(next.rtcp_channel, 5);
    }

    #[test]
    fn next_play_interleaved_channels_allocates_first_free_pair() {
        let mut play_tracks = HashMap::new();
        play_tracks.insert(
            TrackId(1),
            PlayTrackState {
                transport: PlayTransport::TcpInterleaved {
                    rtp_channel: 0,
                    rtcp_channel: 1,
                },
                payload_type: 96,
                seq: 0,
                ssrc: 0,
                packets_sent: 0,
                octets_sent: 0,
                last_rtp_timestamp: 0,
                timestamp_repair_count: 0,
                sdes_sent: false,
                first_raw_timestamp: None,
            },
        );
        let next = next_play_interleaved_channels(&play_tracks, TrackId(2))
            .expect("next play interleaved channels");
        assert_eq!(next.rtp_channel, 2);
        assert_eq!(next.rtcp_channel, 3);
    }
}
