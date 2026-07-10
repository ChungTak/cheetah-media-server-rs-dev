use std::{collections::HashMap, net::IpAddr};

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use bytes::Bytes;
use cheetah_codec::{
    export_media_description, AacAudioSpecificConfig, AacRtpPacketization, CodecExtradata, CodecId,
    MediaKind, TrackId, TrackInfo, TrackReadiness,
};

pub(crate) const RTSP_MP2P_PROBE_TRACK_MARKER: &[u8] = b"rtsp-compat/mp2p-probe/v1";

struct MediaSection {
    media_kind: MediaKind,
    payload_types: Vec<u8>,
    payload_type: u8,
    codec: Option<CodecId>,
    saw_rtpmap: bool,
    aac_rtp_packetization: AacRtpPacketization,
    aac_latm_config_in_band: bool,
    clock_rate: Option<u32>,
    channels: Option<u8>,
    fmtp: Option<String>,
    control: Option<String>,
}

/// Parses `announce SDP` from input.
/// 从输入解析 `announce SDP`。
pub fn parse_announce_sdp(
    body: &str,
) -> Result<(Vec<TrackInfo>, HashMap<String, TrackId>), String> {
    let mut sections = Vec::<MediaSection>::new();

    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("m=") {
            let mut parts = rest.split_whitespace();
            let media = parts.next().unwrap_or_default();
            let _port = parts.next().unwrap_or_default();
            let _proto = parts.next().unwrap_or_default();
            let payload_types: Vec<u8> = parts.filter_map(|v| v.parse::<u8>().ok()).collect();
            let payload_type = payload_types.first().copied().unwrap_or(96);
            let media_kind = match media {
                "video" => MediaKind::Video,
                "audio" => MediaKind::Audio,
                _ => continue,
            };
            let inferred = infer_static_payload_codec(media_kind, payload_type);
            sections.push(MediaSection {
                media_kind,
                payload_types,
                payload_type,
                codec: inferred,
                saw_rtpmap: false,
                aac_rtp_packetization: AacRtpPacketization::Mpeg4Generic,
                aac_latm_config_in_band: false,
                clock_rate: None,
                channels: None,
                fmtp: None,
                control: None,
            });
            continue;
        }

        let Some(current) = sections.last_mut() else {
            continue;
        };

        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            let Some((pt, map)) = parse_payload_attribute(rest) else {
                continue;
            };
            if pt != current.payload_type && !current.payload_types.contains(&pt) {
                continue;
            }
            let mut map_parts = map.split('/');
            let codec_name = map_parts.next().unwrap_or_default().to_ascii_uppercase();
            let clock_rate = map_parts.next().and_then(|v| v.parse::<u32>().ok());
            let channels = map_parts.next().and_then(|v| v.parse::<u8>().ok());

            let parsed_codec = match codec_name.as_str() {
                "H264" => Some(CodecId::H264),
                "H265" | "HEVC" => Some(CodecId::H265),
                "H266" | "VVC" => Some(CodecId::H266),
                "AV1" | "AV01" => Some(CodecId::AV1),
                "VP8" => Some(CodecId::VP8),
                "VP9" => Some(CodecId::VP9),
                "OPUS" => Some(CodecId::Opus),
                "MPEG4-GENERIC" | "MP4A-LATM" => Some(CodecId::AAC),
                "DVI4" | "ADPCM" => Some(CodecId::ADPCM),
                "PCMA" => Some(CodecId::G711A),
                "PCMU" => Some(CodecId::G711U),
                "MPA" => Some(CodecId::MP3),
                "MP2P" | "PS" | "MPEG-PS" => Some(CodecId::Unknown),
                _ => None,
            };
            current.payload_type = pt;
            current.codec = parsed_codec;
            current.saw_rtpmap = true;
            current.aac_rtp_packetization = if codec_name == "MP4A-LATM" {
                AacRtpPacketization::Latm
            } else {
                AacRtpPacketization::Mpeg4Generic
            };
            current.aac_latm_config_in_band = codec_name == "MP4A-LATM";
            current.clock_rate = clock_rate;
            current.channels = channels;
            continue;
        }

        if let Some(rest) = line.strip_prefix("a=fmtp:") {
            let Some((pt, fmtp)) = parse_payload_attribute(rest) else {
                continue;
            };
            if pt != current.payload_type {
                if current.saw_rtpmap || !current.payload_types.contains(&pt) {
                    continue;
                }
                current.payload_type = pt;
                current.codec = infer_static_payload_codec(current.media_kind, pt);
            }
            if fmtp.is_empty() {
                continue;
            }
            current.fmtp = Some(fmtp.to_string());
            if !current.saw_rtpmap {
                if let Some((codec, aac_packetization, aac_latm_config_in_band)) =
                    infer_compat_codec_from_fmtp(current.media_kind, fmtp)
                {
                    current.codec = Some(codec);
                    if codec == CodecId::AAC {
                        current.aac_rtp_packetization = aac_packetization;
                        current.aac_latm_config_in_band = aac_latm_config_in_band;
                    }
                }
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("a=control:") {
            current.control = Some(rest.trim().to_string());
        }
    }

    let mut tracks = Vec::new();
    let mut control_to_track = HashMap::new();

    for (index, media) in sections.into_iter().enumerate() {
        let codec = media
            .codec
            .or_else(|| infer_compat_payloadless_audio_codec(&media));
        let Some(codec) = codec else {
            continue;
        };
        if !matches!(
            codec,
            CodecId::H264
                | CodecId::H265
                | CodecId::H266
                | CodecId::AV1
                | CodecId::VP8
                | CodecId::VP9
                | CodecId::AAC
                | CodecId::Opus
                | CodecId::ADPCM
                | CodecId::G711A
                | CodecId::G711U
                | CodecId::MP2
                | CodecId::MP3
                | CodecId::Unknown
        ) {
            continue;
        }
        let track_id = TrackId((index + 1) as u32);
        let clock_rate = media.clock_rate.unwrap_or(match codec {
            CodecId::H264
            | CodecId::H265
            | CodecId::H266
            | CodecId::AV1
            | CodecId::VP8
            | CodecId::VP9
            | CodecId::MJPEG => 90_000,
            CodecId::AAC => 48_000,
            CodecId::Opus => 48_000,
            CodecId::ADPCM => 8_000,
            CodecId::G711A | CodecId::G711U => 8_000,
            CodecId::MP2 | CodecId::MP3 => 90_000,
            CodecId::Unknown => 90_000,
        });
        let mut track = TrackInfo::new(track_id, media.media_kind, codec, clock_rate);
        track.payload_type = Some(media.payload_type);
        if media.media_kind == MediaKind::Audio {
            track.sample_rate = Some(clock_rate);
            track.channels = media.channels.or_else(|| default_audio_channels(codec));
        }
        if codec == CodecId::AAC {
            track.aac_rtp_packetization = media.aac_rtp_packetization;
            track.aac_latm_config_in_band = media.aac_latm_config_in_band;
        }
        if codec == CodecId::Unknown {
            track.extradata = CodecExtradata::Raw(Bytes::from_static(RTSP_MP2P_PROBE_TRACK_MARKER));
            track.readiness = TrackReadiness::Ready;
        }

        if let Some(fmtp) = media.fmtp.as_ref() {
            apply_fmtp(codec, media.aac_rtp_packetization, fmtp, &mut track);
        }
        track.refresh_readiness();

        let control = media
            .control
            .as_deref()
            .map(normalize_control)
            .unwrap_or_else(|| format!("trackID={index}"));
        control_to_track.insert(control, track_id);
        tracks.push(track);
    }

    if tracks.is_empty() {
        return Err("announce sdp has no supported media tracks".to_string());
    }

    Ok((tracks, control_to_track))
}

pub(crate) fn is_mp2p_probe_track(track: &TrackInfo) -> bool {
    track.codec == CodecId::Unknown
        && matches!(
            &track.extradata,
            CodecExtradata::Raw(marker) if marker.as_ref() == RTSP_MP2P_PROBE_TRACK_MARKER
        )
}

fn parse_payload_attribute(rest: &str) -> Option<(u8, &str)> {
    let trimmed = rest.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let pt_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let (pt, value) = trimmed.split_at(pt_end);
    let payload_type = pt.parse::<u8>().ok()?;
    Some((payload_type, value.trim_start()))
}

/// Builds the `describe SDP`.
/// 构建 `describe SDP`。
pub fn build_describe_sdp(
    base_uri: &str,
    tracks: &[TrackInfo],
) -> (String, HashMap<String, TrackId>) {
    let mut lines = Vec::<String>::new();
    let mut control_to_track = HashMap::new();
    let (addr_family, session_addr) = session_sdp_address(base_uri);

    lines.push("v=0".to_string());
    lines.push(format!("o=- 0 0 IN {addr_family} {session_addr}"));
    lines.push("s=cheetah".to_string());
    lines.push(format!("c=IN {addr_family} {session_addr}"));
    lines.push("t=0 0".to_string());
    lines.push("a=control:*".to_string());

    for (index, track) in tracks.iter().enumerate() {
        let Some(desc) = export_media_description(track) else {
            continue;
        };
        let control = format!("trackID={index}");
        let payload_type = desc.payload_type;

        lines.push(format!("m={} 0 RTP/AVP {payload_type}", desc.media));
        let rtpmap = if let Some(channels) = desc.channels {
            format!(
                "a=rtpmap:{payload_type} {}/{}/{}",
                desc.codec, desc.clock_rate, channels
            )
        } else {
            format!("a=rtpmap:{payload_type} {}/{}", desc.codec, desc.clock_rate)
        };
        lines.push(rtpmap);
        if let Some(fmtp) = desc.fmtp {
            lines.push(format!("a=fmtp:{payload_type} {fmtp}"));
        }
        lines.push(format!("a=control:{control}"));

        control_to_track.insert(control.clone(), track.track_id);
        control_to_track.insert(format!("{base_uri}/{control}"), track.track_id);
    }

    let mut sdp = lines.join("\r\n");
    sdp.push_str("\r\n");
    (sdp, control_to_track)
}

fn session_sdp_address(base_uri: &str) -> (&'static str, String) {
    let Some(host) = extract_rtsp_host(base_uri) else {
        return ("IP4", "0.0.0.0".to_string());
    };
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => ("IP4", v4.to_string()),
        Ok(IpAddr::V6(v6)) => ("IP6", v6.to_string()),
        Err(_) => ("IP4", host.to_string()),
    }
}

fn extract_rtsp_host(uri: &str) -> Option<&str> {
    let source = uri
        .strip_prefix("rtsp://")
        .or_else(|| uri.strip_prefix("rtsps://"))?;
    let authority = source.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return None;
    }
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    if let Some(rest) = host_port.strip_prefix('[') {
        let end = rest.find(']')?;
        return Some(&rest[..end]);
    }
    Some(host_port.split(':').next().unwrap_or(host_port))
}

/// Normalizes the input into `control`.
/// 将输入归一化为 `control`。
pub fn normalize_control(control: &str) -> String {
    let trimmed = control.trim();
    let trimmed = trimmed
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches('/');
    if trimmed.is_empty() {
        return "trackID=0".to_string();
    }
    if let Some(index) = trimmed.rfind('/') {
        return trimmed[index + 1..].to_string();
    }
    trimmed.to_string()
}

fn apply_fmtp(
    codec: CodecId,
    aac_rtp_packetization: AacRtpPacketization,
    fmtp: &str,
    track: &mut TrackInfo,
) {
    match codec {
        CodecId::H264 => {
            let mut sps = Vec::new();
            let mut pps = Vec::new();
            for part in fmtp.split(';') {
                let mut kv = part.splitn(2, '=');
                let key = kv.next().unwrap_or_default().trim();
                let value = kv.next().unwrap_or_default().trim();
                if key.eq_ignore_ascii_case("sprop-parameter-sets") {
                    let mut sets = value.split(',');
                    if let Some(v) = sets.next() {
                        if let Ok(decoded) = BASE64_STANDARD.decode(v.trim()) {
                            sps.push(Bytes::from(decoded));
                        }
                    }
                    if let Some(v) = sets.next() {
                        if let Ok(decoded) = BASE64_STANDARD.decode(v.trim()) {
                            pps.push(Bytes::from(decoded));
                        }
                    }
                }
            }
            if !sps.is_empty() && !pps.is_empty() {
                track.extradata = CodecExtradata::H264 {
                    sps,
                    pps,
                    avcc: None,
                };
            }
        }
        CodecId::H265 => {
            let mut vps = Vec::new();
            let mut sps = Vec::new();
            let mut pps = Vec::new();
            for part in fmtp.split(';') {
                let mut kv = part.splitn(2, '=');
                let key = kv.next().unwrap_or_default().trim();
                let value = kv.next().unwrap_or_default().trim();
                let target = if key.eq_ignore_ascii_case("sprop-vps") {
                    Some(&mut vps)
                } else if key.eq_ignore_ascii_case("sprop-sps") {
                    Some(&mut sps)
                } else if key.eq_ignore_ascii_case("sprop-pps") {
                    Some(&mut pps)
                } else {
                    None
                };
                let Some(target) = target else {
                    continue;
                };
                for set in value.split(',') {
                    let trimmed = set.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(decoded) = BASE64_STANDARD.decode(trimmed) {
                        target.push(Bytes::from(decoded));
                    }
                }
            }
            if !vps.is_empty() || !sps.is_empty() || !pps.is_empty() {
                track.extradata = CodecExtradata::H265 {
                    vps,
                    sps,
                    pps,
                    hvcc: None,
                };
            }
        }
        CodecId::AV1 => {
            for part in fmtp.split(';') {
                let mut kv = part.splitn(2, '=');
                let key = kv.next().unwrap_or_default().trim();
                let value = kv.next().unwrap_or_default().trim();
                if !key.eq_ignore_ascii_case("config") {
                    continue;
                }
                let config = parse_hex(value)
                    .or_else(|| BASE64_STANDARD.decode(value).ok().map(Bytes::from));
                if let Some(config) = config {
                    track.extradata = CodecExtradata::AV1 {
                        sequence_header: Some(config.clone()),
                        codec_config: Some(config),
                    };
                }
            }
        }
        CodecId::AAC => {
            for part in fmtp.split(';') {
                let mut kv = part.splitn(2, '=');
                let key = kv.next().unwrap_or_default().trim();
                let value = kv.next().unwrap_or_default().trim();
                if key.eq_ignore_ascii_case("cpresent") {
                    track.aac_latm_config_in_band = value != "0";
                    continue;
                }
                if key.eq_ignore_ascii_case("config") {
                    if let Some(bytes) = parse_hex(value) {
                        let asc = if matches!(aac_rtp_packetization, AacRtpPacketization::Latm) {
                            parse_latm_stream_mux_config_to_asc(&bytes)
                        } else {
                            Some(bytes)
                        };
                        if let Some(asc) = asc {
                            if let Some(asc_cfg) = AacAudioSpecificConfig::from_bytes(asc.as_ref())
                            {
                                if let Some(sample_rate) =
                                    sampling_frequency_from_index(asc_cfg.sampling_frequency_index)
                                {
                                    track.sample_rate = Some(sample_rate);
                                }
                                if asc_cfg.channel_configuration > 0 {
                                    track.channels = Some(asc_cfg.channel_configuration);
                                }
                            }
                            track.extradata = CodecExtradata::AAC { asc };
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn sampling_frequency_from_index(index: u8) -> Option<u32> {
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

fn infer_static_payload_codec(media_kind: MediaKind, payload_type: u8) -> Option<CodecId> {
    match (media_kind, payload_type) {
        (MediaKind::Audio, 0) => Some(CodecId::G711U),
        (MediaKind::Audio, 5 | 6 | 16 | 17) => Some(CodecId::ADPCM),
        (MediaKind::Audio, 8) => Some(CodecId::G711A),
        (MediaKind::Audio, 14) => Some(CodecId::MP3),
        _ => None,
    }
}

fn infer_compat_payloadless_audio_codec(media: &MediaSection) -> Option<CodecId> {
    if media.media_kind != MediaKind::Audio
        || media.saw_rtpmap
        || media.fmtp.is_some()
        || media.payload_types.len() != 1
        || media.payload_type < 96
    {
        return None;
    }

    // FFmpeg's RTSP muxer emits adpcm_ima_wav as dynamic PT96 without rtpmap.
    // There is no standards-compliant codec signal left, so keep this fallback
    // narrow to the one-payload audio-only shape observed from that sender.
    Some(CodecId::ADPCM)
}

fn infer_compat_codec_from_fmtp(
    media_kind: MediaKind,
    fmtp: &str,
) -> Option<(CodecId, AacRtpPacketization, bool)> {
    let mut has_h264_key = false;
    let mut has_h265_vps_key = false;
    let mut has_av1_profile_hint = false;
    let mut has_aac_config = false;
    let mut has_aac_mode = false;
    let mut has_aac_size_length = false;
    let mut has_latm_cpresent = None::<bool>;

    for part in fmtp.split(';') {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or_default().trim();
        if key.is_empty() {
            continue;
        }
        let value = kv.next().unwrap_or_default().trim();
        if key.eq_ignore_ascii_case("sprop-parameter-sets")
            || key.eq_ignore_ascii_case("packetization-mode")
        {
            has_h264_key = true;
        }
        if key.eq_ignore_ascii_case("sprop-vps") {
            has_h265_vps_key = true;
        }
        if key.eq_ignore_ascii_case("profile") || key.eq_ignore_ascii_case("level-idx") {
            has_av1_profile_hint = true;
        }
        if key.eq_ignore_ascii_case("config") {
            has_aac_config = true;
        }
        if key.eq_ignore_ascii_case("mode") && value.eq_ignore_ascii_case("AAC-hbr") {
            has_aac_mode = true;
        }
        if key.eq_ignore_ascii_case("sizelength")
            || key.eq_ignore_ascii_case("indexlength")
            || key.eq_ignore_ascii_case("indexdeltalength")
        {
            has_aac_size_length = true;
        }
        if key.eq_ignore_ascii_case("cpresent") {
            has_latm_cpresent = Some(value != "0");
        }
    }

    match media_kind {
        MediaKind::Video => {
            if has_h265_vps_key {
                Some((CodecId::H265, AacRtpPacketization::Mpeg4Generic, false))
            } else if has_h264_key {
                Some((CodecId::H264, AacRtpPacketization::Mpeg4Generic, false))
            } else if has_av1_profile_hint {
                Some((CodecId::AV1, AacRtpPacketization::Mpeg4Generic, false))
            } else {
                None
            }
        }
        MediaKind::Audio => {
            if let Some(in_band) = has_latm_cpresent {
                Some((CodecId::AAC, AacRtpPacketization::Latm, in_band))
            } else if has_aac_mode || has_aac_size_length || has_aac_config {
                Some((CodecId::AAC, AacRtpPacketization::Mpeg4Generic, false))
            } else {
                None
            }
        }
        MediaKind::Data | MediaKind::Subtitle => None,
    }
}

fn default_audio_channels(codec: CodecId) -> Option<u8> {
    match codec {
        CodecId::ADPCM | CodecId::G711A | CodecId::G711U => Some(1),
        CodecId::Opus => Some(2),
        _ => None,
    }
}

fn parse_latm_stream_mux_config_to_asc(config: &[u8]) -> Option<Bytes> {
    let mut bits = BitReader::new(config);
    let audio_mux_version = bits.read_bit()?;
    let audio_mux_version_a = if audio_mux_version == 1 {
        bits.read_bit()?
    } else {
        0
    };
    if audio_mux_version_a != 0 {
        return None;
    }
    if audio_mux_version == 1 {
        let _tara_buffer_fullness = latm_get_value(&mut bits)?;
    }
    let all_streams_same_time_framing = bits.read_bit()?;
    if all_streams_same_time_framing == 0 {
        return None;
    }
    let num_sub_frames = bits.read_bits(6)?;
    if num_sub_frames != 0 {
        return None;
    }
    let num_programs = bits.read_bits(4)?;
    if num_programs != 0 {
        return None;
    }
    let num_layers = bits.read_bits(3)?;
    if num_layers != 0 {
        return None;
    }

    let (asc_start, asc_end) = if audio_mux_version == 0 {
        let asc_start = bits.bit_offset();
        let asc_tail = bits_to_bytes(config, asc_start, config.len().saturating_mul(8))?;
        let mut asc_bits = BitReader::new(asc_tail.as_ref());
        parse_audio_specific_config(&mut asc_bits)?;
        (asc_start, asc_start.checked_add(asc_bits.bit_offset())?)
    } else {
        let asc_len_bits = usize::try_from(latm_get_value(&mut bits)?).ok()?;
        if asc_len_bits == 0 {
            return None;
        }
        let asc_start = bits.bit_offset();
        bits.skip_bits(asc_len_bits)?;
        (asc_start, bits.bit_offset())
    };

    let asc = bits_to_bytes(config, asc_start, asc_end)?;
    AacAudioSpecificConfig::from_bytes(asc.as_ref())?;
    let mut asc_bits = BitReader::new(asc.as_ref());
    parse_audio_specific_config(&mut asc_bits)?;
    if asc
        .len()
        .saturating_mul(8)
        .saturating_sub(asc_bits.bit_offset())
        > 7
    {
        return None;
    }

    Some(asc)
}

fn latm_get_value(bits: &mut BitReader<'_>) -> Option<u32> {
    let bytes_for_value = (bits.read_bits(2)? as usize) + 1;
    let mut value = 0u32;
    for _ in 0..bytes_for_value {
        value = (value << 8) | bits.read_bits(8)?;
    }
    Some(value)
}

fn parse_audio_specific_config(bits: &mut BitReader<'_>) -> Option<()> {
    parse_audio_specific_config_without_sync_extension(bits)?;

    // Optional sync extension (e.g. SBR/PS) may append extra bits to ASC.
    if bits.remaining_bits() >= 11 && bits.peek_bits(11)? == 0x2B7 {
        bits.skip_bits(11)?;
        let ext_object_type = get_audio_object_type(bits)?;
        if ext_object_type == 5 {
            bits.skip_bits(1)?; // sbrPresentFlag
            let ext_sampling_frequency_index = bits.read_bits(4)?;
            if ext_sampling_frequency_index == 0x0F {
                bits.skip_bits(24)?;
            }
            if bits.remaining_bits() >= 11 && bits.peek_bits(11)? == 0x548 {
                bits.skip_bits(11)?; // syncExtensionType for PS
                bits.skip_bits(1)?; // psPresentFlag
            }
        }
    }

    Some(())
}

fn parse_audio_specific_config_without_sync_extension(bits: &mut BitReader<'_>) -> Option<()> {
    let audio_object_type = get_audio_object_type(bits)?;
    let sampling_frequency_index = bits.read_bits(4)?;
    if sampling_frequency_index == 0x0f {
        bits.skip_bits(24)?;
    }
    let channel_configuration = bits.read_bits(4)? as u8;

    let mut object_type_for_ga = audio_object_type;
    if audio_object_type == 5 || audio_object_type == 29 {
        let extension_sampling_frequency_index = bits.read_bits(4)?;
        if extension_sampling_frequency_index == 0x0f {
            bits.skip_bits(24)?;
        }
        object_type_for_ga = get_audio_object_type(bits)?;
        if object_type_for_ga == 22 {
            bits.skip_bits(4)?;
        }
    }

    match object_type_for_ga {
        1 | 2 | 3 | 4 | 6 | 7 | 17 | 19 | 20 | 21 | 22 | 23 => {
            parse_ga_specific_config(bits, object_type_for_ga, channel_configuration)?
        }
        _ => return None,
    }

    if matches!(object_type_for_ga, 17 | 19 | 20 | 21 | 22 | 23) {
        bits.skip_bits(2)?; // epConfig
    }

    Some(())
}

fn get_audio_object_type(bits: &mut BitReader<'_>) -> Option<u8> {
    let object_type = bits.read_bits(5)? as u8;
    if object_type == 31 {
        let extended = bits.read_bits(6)? as u8;
        return Some(32 + extended);
    }
    Some(object_type)
}

fn parse_ga_specific_config(
    bits: &mut BitReader<'_>,
    object_type: u8,
    channel_configuration: u8,
) -> Option<()> {
    bits.skip_bits(1)?; // frameLengthFlag
    if bits.read_bit()? == 1 {
        bits.skip_bits(14)?; // coreCoderDelay
    }
    let extension_flag = bits.read_bit()?;

    if channel_configuration == 0 {
        parse_program_config_element(bits)?;
    }

    if matches!(object_type, 6 | 20) {
        bits.skip_bits(3)?; // layerNr
    }

    if extension_flag == 1 {
        if object_type == 22 {
            bits.skip_bits(5 + 11)?; // numOfSubFrame + layer_length
        }
        if matches!(object_type, 17 | 19 | 20 | 23) {
            bits.skip_bits(3)?; // resilience flags
        }
        bits.skip_bits(1)?; // extensionFlag3
    }

    Some(())
}

fn parse_program_config_element(bits: &mut BitReader<'_>) -> Option<()> {
    bits.skip_bits(4 + 2 + 4)?;
    let num_front = bits.read_bits(4)? as usize;
    let num_side = bits.read_bits(4)? as usize;
    let num_back = bits.read_bits(4)? as usize;
    let num_lfe = bits.read_bits(2)? as usize;
    let num_assoc_data = bits.read_bits(3)? as usize;
    let num_valid_cc = bits.read_bits(4)? as usize;

    if bits.read_bit()? == 1 {
        bits.skip_bits(4)?; // mono_mixdown_element_number
    }
    if bits.read_bit()? == 1 {
        bits.skip_bits(4)?; // stereo_mixdown_element_number
    }
    if bits.read_bit()? == 1 {
        bits.skip_bits(3)?; // matrix mixdown index + pseudo-surround enable
    }

    for _ in 0..num_front {
        bits.skip_bits(5)?; // is_cpe + tag_select
    }
    for _ in 0..num_side {
        bits.skip_bits(5)?;
    }
    for _ in 0..num_back {
        bits.skip_bits(5)?;
    }
    for _ in 0..num_lfe {
        bits.skip_bits(4)?;
    }
    for _ in 0..num_assoc_data {
        bits.skip_bits(4)?;
    }
    for _ in 0..num_valid_cc {
        bits.skip_bits(5)?; // is_ind_sw + tag_select
    }

    bits.align_to_byte()?;
    let comment_bytes = bits.read_bits(8)? as usize;
    bits.skip_bits(comment_bytes.saturating_mul(8))?;
    Some(())
}

fn bits_to_bytes(data: &[u8], start_bit: usize, end_bit: usize) -> Option<Bytes> {
    if end_bit <= start_bit {
        return None;
    }
    if end_bit > data.len().saturating_mul(8) {
        return None;
    }
    let bit_len = end_bit - start_bit;
    let mut out = vec![0u8; bit_len.div_ceil(8)];
    for i in 0..bit_len {
        let src_bit = start_bit + i;
        let src_byte = *data.get(src_bit / 8)?;
        let src_shift = 7usize.saturating_sub(src_bit % 8);
        let bit = (src_byte >> src_shift) & 1;
        out[i / 8] |= bit << (7usize.saturating_sub(i % 8));
    }
    Some(Bytes::from(out))
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_offset: 0,
        }
    }

    fn bit_offset(&self) -> usize {
        self.bit_offset
    }

    fn read_bit(&mut self) -> Option<u8> {
        let byte_index = self.bit_offset / 8;
        let bit_in_byte = 7usize.saturating_sub(self.bit_offset % 8);
        let byte = *self.data.get(byte_index)?;
        self.bit_offset += 1;
        Some((byte >> bit_in_byte) & 1)
    }

    fn read_bits(&mut self, n: usize) -> Option<u32> {
        if n > 32 {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..n {
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Some(value)
    }

    fn skip_bits(&mut self, n: usize) -> Option<()> {
        if self.bit_offset.checked_add(n)? > self.data.len().saturating_mul(8) {
            return None;
        }
        self.bit_offset += n;
        Some(())
    }

    fn peek_bits(&self, n: usize) -> Option<u32> {
        let mut clone = Self {
            data: self.data,
            bit_offset: self.bit_offset,
        };
        clone.read_bits(n)
    }

    fn remaining_bits(&self) -> usize {
        self.data
            .len()
            .saturating_mul(8)
            .saturating_sub(self.bit_offset)
    }

    fn align_to_byte(&mut self) -> Option<()> {
        let rem = self.bit_offset % 8;
        if rem == 0 {
            return Some(());
        }
        self.skip_bits(8 - rem)
    }
}

fn parse_hex(raw: &str) -> Option<Bytes> {
    if !raw.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(raw.len() / 2);
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let hi = decode_hex_nibble(bytes[i])?;
        let lo = decode_hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(Bytes::from(out))
}

fn decode_hex_nibble(v: u8) -> Option<u8> {
    match v {
        b'0'..=b'9' => Some(v - b'0'),
        b'a'..=b'f' => Some(v - b'a' + 10),
        b'A'..=b'F' => Some(v - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_announce_sdp_with_h264_and_aac() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   t=0 0\r\n\
                   m=video 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 H264/90000\r\n\
                   a=fmtp:96 packetization-mode=1;sprop-parameter-sets=Z0IAH5WoFAFuQA==,aM4G4g==\r\n\
                   a=control:trackID=0\r\n\
                   m=audio 0 RTP/AVP 97\r\n\
                   a=rtpmap:97 MPEG4-GENERIC/48000/2\r\n\
                   a=fmtp:97 profile-level-id=1;mode=AAC-hbr;config=1190\r\n\
                   a=control:trackID=1\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 2);
        assert!(control.contains_key("trackID=0"));
        assert!(control.contains_key("trackID=1"));
    }

    #[test]
    fn parses_announce_sdp_with_latm_aac_control_stream() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=Session streamed with GStreamer\r\n\
                   t=0 0\r\n\
                   m=audio 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 MP4A-LATM/48000\r\n\
                   a=fmtp:96 cpresent=0;config=40002300099088004001881898c2ecc66c625c665c626060adca00\r\n\
                   a=control:stream=1\r\n\
                   m=video 0 RTP/AVP 99\r\n\
                   a=rtpmap:99 H264/90000\r\n\
                   a=fmtp:99 packetization-mode=1;sprop-parameter-sets=Z2QAKKzZQHgCJ+XARAAAAwAEAAADAPA8YMZY,aOvjyyLA\r\n\
                   a=control:stream=0\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 2);
        assert!(control.contains_key("stream=0"));
        assert!(control.contains_key("stream=1"));

        let audio_track = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Audio)
            .expect("audio track");
        assert_eq!(audio_track.aac_rtp_packetization, AacRtpPacketization::Latm);
        assert!(!audio_track.aac_latm_config_in_band);
        let CodecExtradata::AAC { asc } = &audio_track.extradata else {
            panic!("expected aac extradata");
        };
        assert_eq!(
            asc.as_ref(),
            &[
                0x11, 0x80, 0x04, 0xC8, 0x44, 0x00, 0x20, 0x00, 0xC4, 0x0C, 0x4C, 0x61, 0x76, 0x63,
                0x36, 0x31, 0x2E, 0x33, 0x2E, 0x31, 0x30, 0x30, 0x56, 0xE5, 0x00
            ]
        );
    }

    #[test]
    fn parses_latm_stream_mux_config_to_audio_specific_config() {
        let latm = parse_hex("40002300099088004001881898c2ecc66c625c665c626060adca00")
            .expect("latm config");
        let asc = parse_latm_stream_mux_config_to_asc(latm.as_ref()).expect("asc from latm");
        assert_eq!(
            asc.as_ref(),
            &[
                0x11, 0x80, 0x04, 0xC8, 0x44, 0x00, 0x20, 0x00, 0xC4, 0x0C, 0x4C, 0x61, 0x76, 0x63,
                0x36, 0x31, 0x2E, 0x33, 0x2E, 0x31, 0x30, 0x30, 0x56, 0xE5, 0x00
            ]
        );
    }

    #[test]
    fn parses_audio_specific_config_with_program_config_element() {
        let asc = parse_hex("118004c844002000c40c4c61766336312e332e31303056e500").expect("asc");
        let mut bits = BitReader::new(asc.as_ref());
        parse_audio_specific_config(&mut bits).expect("parse asc");
        assert!(bits.bit_offset() <= asc.len() * 8);
    }

    fn push_test_bits(bits: &mut Vec<u8>, value: u32, width: usize) {
        for shift in (0..width).rev() {
            bits.push(((value >> shift) & 1) as u8);
        }
    }

    fn pack_test_bits(bits: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; bits.len().div_ceil(8)];
        for (i, bit) in bits.iter().enumerate() {
            out[i / 8] |= *bit << (7usize.saturating_sub(i % 8));
        }
        out
    }

    #[test]
    fn parses_latm_audio_mux_version_one_audio_specific_config() {
        let mut bits = Vec::new();
        push_test_bits(&mut bits, 1, 1);
        push_test_bits(&mut bits, 0, 1);
        push_test_bits(&mut bits, 0, 2);
        push_test_bits(&mut bits, 0, 8);
        push_test_bits(&mut bits, 1, 1);
        push_test_bits(&mut bits, 0, 6);
        push_test_bits(&mut bits, 0, 4);
        push_test_bits(&mut bits, 0, 3);
        push_test_bits(&mut bits, 0, 2);
        push_test_bits(&mut bits, 16, 8);
        push_test_bits(&mut bits, 0x11, 8);
        push_test_bits(&mut bits, 0x90, 8);

        let latm = pack_test_bits(&bits);
        let asc =
            parse_latm_stream_mux_config_to_asc(latm.as_ref()).expect("asc from latm version 1");
        assert_eq!(asc.as_ref(), &[0x11, 0x90]);
    }

    #[test]
    fn latm_invalid_audio_mux_version_a_does_not_set_aac_extradata() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=Session\r\n\
                   t=0 0\r\n\
                   m=audio 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 MP4A-LATM/48000/2\r\n\
                   a=fmtp:96 cpresent=0;config=C0\r\n\
                   a=control:stream=1\r\n";

        let (tracks, _control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        let audio_track = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Audio)
            .expect("audio track");
        assert!(matches!(audio_track.extradata, CodecExtradata::None));
    }

    #[test]
    fn latm_without_cpresent_defaults_to_in_band_config() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=Session\r\n\
                   t=0 0\r\n\
                   m=audio 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 MP4A-LATM/48000/2\r\n\
                   a=fmtp:96 profile-level-id=1\r\n\
                   a=control:stream=1\r\n";

        let (tracks, _control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        let audio_track = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Audio)
            .expect("audio track");
        assert!(audio_track.aac_latm_config_in_band);
    }

    #[test]
    fn parses_announce_sdp_with_h265_track_and_parameter_sets() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=video 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 H265/90000\r\n\
                   a=fmtp:96 sprop-vps=QAEMAf//AUAAAAMAgAAAAwAAAwB7rAk=; sprop-sps=QgEBAUAAAAMAgAAAAwAAAwB7oAPAgBEFlrksrZrlUTYAgA==; sprop-pps=RAHA4w8DMkA=\r\n\
                   a=control:trackID=0\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("trackID=0"));
        assert_eq!(tracks[0].codec, CodecId::H265);
        let CodecExtradata::H265 { vps, sps, pps, .. } = &tracks[0].extradata else {
            panic!("expected h265 extradata");
        };
        assert!(!vps.is_empty());
        assert!(!sps.is_empty());
        assert!(!pps.is_empty());
    }

    #[test]
    fn parses_announce_sdp_with_av1_track() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=video 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 AV1/90000\r\n\
                   a=fmtp:96 profile=0;level-idx=9;tier=0\r\n\
                   a=control:trackID=0\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("trackID=0"));
        assert_eq!(tracks[0].codec, CodecId::AV1);
    }

    #[test]
    fn parses_announce_sdp_with_static_pcma_payload_type() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   a=tool:libavformat 62.3.100\r\n\
                   m=video 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 H265/90000\r\n\
                   a=fmtp:96 sprop-vps=QAEMAf//AUAAAAMAgAAAAwAAAwB7rAk=; sprop-sps=QgEBAUAAAAMAgAAAAwAAAwB7oAPAgBEFlrksrZrlUTYAgA==; sprop-pps=RAHA4w8DMkA=\r\n\
                   a=control:streamid=0\r\n\
                   m=audio 0 RTP/AVP 8\r\n\
                   a=control:streamid=1\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 2);
        assert!(control.contains_key("streamid=0"));
        assert!(control.contains_key("streamid=1"));
        let audio = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Audio)
            .expect("audio track");
        assert_eq!(audio.codec, CodecId::G711A);
        assert_eq!(audio.clock_rate, 8_000);
    }

    #[test]
    fn rejects_static_payload_when_explicit_rtpmap_is_unsupported() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=audio 0 RTP/AVP 8\r\n\
                   a=rtpmap:8 X-UNKNOWN/8000\r\n\
                   a=control:streamid=1\r\n";

        let err = parse_announce_sdp(sdp).expect_err("unsupported explicit rtpmap should fail");
        assert!(err.contains("no supported media tracks"));
    }

    #[test]
    fn ignores_rtpmap_when_payload_type_does_not_match_current_media_section() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=audio 0 RTP/AVP 8\r\n\
                   a=rtpmap:96 OPUS/48000/2\r\n\
                   a=control:streamid=1\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("streamid=1"));
        let audio = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Audio)
            .expect("audio track");
        assert_eq!(audio.codec, CodecId::G711A);
        assert_eq!(audio.clock_rate, 8_000);
    }

    #[test]
    fn parses_non_first_payload_type_from_rtpmap_in_multi_pt_audio_section() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=audio 0 RTP/AVP 8 97\r\n\
                   a=rtpmap:97 OPUS/48000/2\r\n\
                   a=control:streamid=1\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("streamid=1"));
        let audio = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Audio)
            .expect("audio track");
        assert_eq!(audio.codec, CodecId::Opus);
        assert_eq!(audio.payload_type, Some(97));
        assert_eq!(audio.clock_rate, 48_000);
        assert_eq!(audio.channels, Some(2));
    }

    #[test]
    fn parses_dvi4_adpcm_audio_track() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=audio 0 RTP/AVP 5 97\r\n\
                   a=rtpmap:97 DVI4/8000/1\r\n\
                   a=control:streamid=1\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("streamid=1"));
        let audio = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Audio)
            .expect("audio track");
        assert_eq!(audio.codec, CodecId::ADPCM);
        assert_eq!(audio.payload_type, Some(97));
        assert_eq!(audio.clock_rate, 8_000);
        assert_eq!(audio.channels, Some(1));
    }

    #[test]
    fn infers_ffmpeg_dynamic_adpcm_audio_without_rtpmap() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   a=tool:libavformat 62.3.100\r\n\
                   m=audio 0 RTP/AVP 96\r\n\
                   b=AS:128\r\n\
                   a=control:streamid=0\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("streamid=0"));
        let audio = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Audio)
            .expect("audio track");
        assert_eq!(audio.codec, CodecId::ADPCM);
        assert_eq!(audio.payload_type, Some(96));
        assert_eq!(audio.clock_rate, 8_000);
        assert_eq!(audio.sample_rate, Some(8_000));
        assert_eq!(audio.channels, Some(1));
    }

    #[test]
    fn parses_non_first_payload_type_from_rtpmap_in_multi_pt_video_section() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=video 0 RTP/AVP 96 98\r\n\
                   a=rtpmap:98 H265/90000\r\n\
                   a=control:streamid=0\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("streamid=0"));
        let video = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Video)
            .expect("video track");
        assert_eq!(video.codec, CodecId::H265);
        assert_eq!(video.payload_type, Some(98));
        assert_eq!(video.clock_rate, 90_000);
    }

    #[test]
    fn parses_h264_track_without_fmtp_and_with_absolute_control_uri() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 10.0.0.8\r\n\
                   s=No Name\r\n\
                   c=IN IP4 10.0.0.8\r\n\
                   t=0 0\r\n\
                   m=video 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 H264/90000\r\n\
                   a=control:rtsp://10.0.0.8:554/cam/trackID=1?ctype=video\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("trackID=1"));
        let video = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Video)
            .expect("video track");
        assert_eq!(video.codec, CodecId::H264);
        assert!(matches!(video.extradata, CodecExtradata::None));
    }

    #[test]
    fn infers_video_codec_from_fmtp_without_rtpmap_and_fallbacks_payload_type() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 10.0.0.9\r\n\
                   s=No Name\r\n\
                   c=IN IP4 10.0.0.9\r\n\
                   t=0 0\r\n\
                   m=video 0 RTP/AVP 96 98\r\n\
                   a=fmtp:98 packetization-mode=1; sprop-parameter-sets=Z0IAH5WoFAFuQA==,aM4G4g==\r\n\
                   a=control:rtsp://10.0.0.9:554/stream/main/trackID=98?ctype=video\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("trackID=98"));
        let video = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Video)
            .expect("video track");
        assert_eq!(video.codec, CodecId::H264);
        assert_eq!(video.payload_type, Some(98));
        let CodecExtradata::H264 { sps, pps, .. } = &video.extradata else {
            panic!("expected h264 extradata from fmtp fallback");
        };
        assert!(!sps.is_empty());
        assert!(!pps.is_empty());
    }

    #[test]
    fn ignores_fmtp_when_payload_type_does_not_match_current_media_section() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=audio 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 MPEG4-GENERIC/48000/2\r\n\
                   a=fmtp:97 profile-level-id=1;mode=AAC-hbr;config=1190\r\n\
                   a=control:streamid=1\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("streamid=1"));
        let audio = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Audio)
            .expect("audio track");
        assert_eq!(audio.codec, CodecId::AAC);
        assert!(matches!(audio.extradata, CodecExtradata::None));
    }

    #[test]
    fn parses_announce_sdp_with_vp8_and_opus_tracks() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=video 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 VP8/90000\r\n\
                   a=control:streamid=0\r\n\
                   m=audio 0 RTP/AVP 97\r\n\
                   a=rtpmap:97 OPUS/48000/2\r\n\
                   a=control:streamid=1\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 2);
        assert!(control.contains_key("streamid=0"));
        assert!(control.contains_key("streamid=1"));
        let video = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Video)
            .expect("video track");
        let audio = tracks
            .iter()
            .find(|track| track.media_kind == MediaKind::Audio)
            .expect("audio track");
        assert_eq!(video.codec, CodecId::VP8);
        assert_eq!(video.clock_rate, 90_000);
        assert_eq!(audio.codec, CodecId::Opus);
        assert_eq!(audio.clock_rate, 48_000);
        assert_eq!(audio.channels, Some(2));
    }

    #[test]
    fn malformed_sdp_lines_are_ignored_when_supported_tracks_exist() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=video 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 H264/90000\r\n\
                   a=fmtp:96 profile-level-id=42e01f;sprop-parameter-sets=Z0LgHtoCgPaE,aM4G4g==\r\n\
                   a=this-is-a-malformed-line-without-meaning\r\n\
                   x-garbage-line-without-prefix\r\n\
                   a=control:trackID=0\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("trackID=0"));
        assert_eq!(tracks[0].codec, CodecId::H264);
    }

    #[test]
    fn parses_mp2p_track_as_unknown_compat_probe_track() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=No Name\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=video 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 MP2P/90000\r\n\
                   a=control:streamid=0\r\n";

        let (tracks, control) = parse_announce_sdp(sdp).expect("parse announce sdp");
        assert_eq!(tracks.len(), 1);
        assert!(control.contains_key("streamid=0"));
        let track = &tracks[0];
        assert_eq!(track.codec, CodecId::Unknown);
        assert_eq!(track.payload_type, Some(96));
    }

    #[test]
    fn build_describe_sdp_uses_base_uri_host_and_connection_line() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::AV1, 90_000);
        track.readiness = cheetah_codec::TrackReadiness::Ready;

        let (sdp, control_map) =
            build_describe_sdp("rtsp://192.168.111.183:8554/live/test", &[track]);
        assert!(sdp.contains("o=- 0 0 IN IP4 192.168.111.183"));
        assert!(sdp.contains("c=IN IP4 192.168.111.183"));
        assert!(sdp.contains("a=fmtp:96 profile=0;level-idx=9;tier=0"));
        assert!(control_map.contains_key("trackID=0"));
        assert!(control_map.contains_key("rtsp://192.168.111.183:8554/live/test/trackID=0"));
    }

    #[test]
    fn build_describe_sdp_uses_domain_host_without_fallback_zero() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::AV1, 90_000);
        track.readiness = cheetah_codec::TrackReadiness::Ready;

        let (sdp, _control_map) = build_describe_sdp("rtsp://example.com/live/test", &[track]);

        assert!(sdp.contains("o=- 0 0 IN IP4 example.com"));
        assert!(sdp.contains("c=IN IP4 example.com"));
        assert!(!sdp.contains("o=- 0 0 IN IP4 0.0.0.0"));
        assert!(!sdp.contains("c=IN IP4 0.0.0.0"));
    }
}
