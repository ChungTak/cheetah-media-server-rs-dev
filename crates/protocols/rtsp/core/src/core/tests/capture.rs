use bytes::Bytes;

use super::{
    CoreInput, CoreOutput, RtcpPacket, RtpPacket, RtspCore, RtspEvent, RtspMethod, RtspRequest,
    RtspTransport, Sdp,
};

const FLAG_STANDARD_ASSERTABLE: u8 = 0x01;
const FLAG_PROBE_ONLY: u8 = 0x02;
const FLAG_TRUNCATED_PREFIX: u8 = 0x04;
const KNOWN_FLAGS: u8 = FLAG_STANDARD_ASSERTABLE | FLAG_PROBE_ONLY | FLAG_TRUNCATED_PREFIX;

const KIND_RTSP_TCP_C2S: u8 = 1;
const KIND_RTSP_TCP_S2C: u8 = 2;
const MAX_RECORDS_PER_FIXTURE: usize = 32_768;

const H264_TCP_PUBLISH_PLAY: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtsp-capture/standard/h264_tcp_publish_play.rtspcap"
);
const H264_UDP_PUBLISH_PLAY: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtsp-capture/standard/h264_udp_publish_play.rtspcap"
);
const H265_TCP_PUBLISH_PLAY: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtsp-capture/standard/h265_tcp_publish_play.rtspcap"
);
const AUDIO_ONLY_UDP_PUBLISH_PLAY: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtsp-capture/standard/audio_only_udp_publish_play.rtspcap"
);
const AV1_PROBE: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtsp-capture/probes/av1_probe.rtspcap"
);
const VP8_PROBE: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtsp-capture/probes/vp8_probe.rtspcap"
);
const VP9_PROBE: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtsp-capture/probes/vp9_probe.rtspcap"
);
const H266_PROBE: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtsp-capture/probes/h266_probe.rtspcap"
);
const HIGH_BITRATE_PROBE: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtsp-capture/probes/high_bitrate_probe.rtspcap"
);

const KIND_UDP_PUBLISH_RTP: u8 = 3;
const KIND_UDP_PLAY_RTP: u8 = 5;
const KIND_TCP_INTERLEAVED_RTP: u8 = 7;

const MAX_ROBUSTNESS_RECORDS: usize = 256;
const MAX_VIEW_EVENTS: usize = 8_192;

#[derive(Debug, Clone)]
struct RawRecord {
    kind: u8,
    flags: u8,
    payload: Vec<u8>,
}

#[derive(Debug, Clone)]
struct CaptureFixture {
    case: &'static str,
    bytes: &'static [u8],
}

#[derive(Debug, Clone, Copy)]
enum TcpInputView {
    OriginalRecords,
    SingleBuffer,
    OneByteChunks,
}

#[derive(Debug)]
enum DecodeError {
    Truncated(&'static str),
    BadMagic,
    InvalidFlags(u8),
    InvalidKind(u8),
    ZeroLengthPayload(usize),
    ExcessiveRecordCount(u32),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated(ctx) => write!(f, "truncated while reading {ctx}"),
            Self::BadMagic => write!(f, "invalid rtspcap magic"),
            Self::InvalidFlags(flags) => write!(f, "invalid flags 0x{flags:02x}"),
            Self::InvalidKind(kind) => write!(f, "invalid record kind {kind}"),
            Self::ZeroLengthPayload(index) => write!(f, "zero-length payload at record {index}"),
            Self::ExcessiveRecordCount(count) => {
                write!(f, "record count {count} exceeds safety limit")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

#[test]
fn standard_capture_replay_publish_and_play_sequences_across_views() {
    let fixtures = [
        CaptureFixture {
            case: "h264_tcp_publish_play",
            bytes: H264_TCP_PUBLISH_PLAY,
        },
        CaptureFixture {
            case: "h264_udp_publish_play",
            bytes: H264_UDP_PUBLISH_PLAY,
        },
    ];

    for fixture in fixtures {
        let records = decode_rtspcap(fixture.bytes).expect("fixture decode should succeed");
        let control_payloads = c2s_control_payloads(&records);
        assert!(
            !control_payloads.is_empty(),
            "fixture {} must contain C2S RTSP control payloads",
            fixture.case
        );

        for view in [
            TcpInputView::OriginalRecords,
            TcpInputView::SingleBuffer,
            TcpInputView::OneByteChunks,
        ] {
            let requests =
                replay_request_events(&control_payloads, view).expect("replay should not fail");
            assert!(
                !requests.is_empty(),
                "fixture {} with view {:?} should emit request events",
                fixture.case,
                view
            );

            assert_request_semantics(fixture.case, view, &requests);

            let methods: Vec<RtspMethod> = requests.iter().map(|req| req.method.clone()).collect();
            assert!(
                contains_subsequence(
                    &methods,
                    &[
                        RtspMethod::Options,
                        RtspMethod::Announce,
                        RtspMethod::Setup,
                        RtspMethod::Record,
                    ],
                ),
                "fixture {} with view {:?} must include publish sequence",
                fixture.case,
                view
            );
            assert!(
                contains_subsequence(
                    &methods,
                    &[
                        RtspMethod::Options,
                        RtspMethod::Describe,
                        RtspMethod::Setup,
                        RtspMethod::Play
                    ],
                ),
                "fixture {} with view {:?} must include play sequence",
                fixture.case,
                view
            );
        }
    }
}

fn assert_request_semantics(case: &str, view: TcpInputView, requests: &[RtspRequest]) {
    let mut has_announce = false;
    let mut has_setup = false;

    for req in requests {
        assert!(
            req.cseq.is_some(),
            "case {case} view {:?} request {} must include parseable CSeq",
            view,
            req.method
        );

        if req.method == RtspMethod::Announce {
            has_announce = true;
            assert!(
                !req.body.is_empty(),
                "case {case} view {:?} ANNOUNCE body must not be empty",
                view
            );
            let body = std::str::from_utf8(&req.body).expect("ANNOUNCE body must be utf-8 SDP");
            Sdp::parse(body).expect("ANNOUNCE SDP must parse");
        }

        if req.method == RtspMethod::Setup {
            has_setup = true;
            let transport = header_value(req, "Transport")
                .expect("SETUP request must include Transport header");
            let parsed =
                RtspTransport::parse_multiple(transport).expect("SETUP Transport must parse");
            assert!(
                !parsed.is_empty(),
                "SETUP Transport should yield at least one transport spec"
            );
        }
    }

    assert!(
        has_announce,
        "case {case} view {:?} should include ANNOUNCE request",
        view
    );
    assert!(
        has_setup,
        "case {case} view {:?} should include SETUP request",
        view
    );
}

fn header_value<'a>(request: &'a RtspRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

fn contains_subsequence(methods: &[RtspMethod], expected: &[RtspMethod]) -> bool {
    if expected.is_empty() {
        return true;
    }
    let mut idx = 0;
    for method in methods {
        if method == &expected[idx] {
            idx += 1;
            if idx == expected.len() {
                return true;
            }
        }
    }
    false
}

fn replay_request_events(
    payloads: &[Vec<u8>],
    view: TcpInputView,
) -> Result<Vec<RtspRequest>, super::RtspCoreError> {
    let chunks = match view {
        TcpInputView::OriginalRecords => payloads
            .iter()
            .map(|payload| Bytes::copy_from_slice(payload))
            .collect(),
        TcpInputView::SingleBuffer => vec![Bytes::copy_from_slice(&payloads.concat())],
        TcpInputView::OneByteChunks => payloads
            .iter()
            .flat_map(|payload| payload.iter().map(|byte| Bytes::copy_from_slice(&[*byte])))
            .collect(),
    };

    let mut core = RtspCore::new();
    let mut requests = Vec::new();
    for chunk in chunks {
        let outputs = core.handle_input(CoreInput::Bytes(chunk))?;
        for output in outputs {
            if let CoreOutput::Event(RtspEvent::Request(request)) = output {
                requests.push(request);
            }
        }
    }
    Ok(requests)
}

fn c2s_control_payloads(records: &[RawRecord]) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| record.kind == KIND_RTSP_TCP_C2S)
        .filter(|record| record.flags & KNOWN_FLAGS != 0)
        .filter(|record| {
            let first = record.payload.first().copied().unwrap_or_default();
            (first as char).is_ascii_uppercase()
        })
        .map(|record| record.payload.clone())
        .collect()
}

fn decode_rtspcap(bytes: &[u8]) -> Result<Vec<RawRecord>, DecodeError> {
    if bytes.len() < 8 {
        return Err(DecodeError::Truncated("header"));
    }
    if &bytes[..4] != b"RSF1" {
        return Err(DecodeError::BadMagic);
    }

    let mut cursor = 4usize;
    let record_count = read_u32(bytes, &mut cursor, "record_count")?;
    if record_count as usize > MAX_RECORDS_PER_FIXTURE {
        return Err(DecodeError::ExcessiveRecordCount(record_count));
    }

    let mut out = Vec::with_capacity(record_count as usize);
    for index in 0..(record_count as usize) {
        let kind = read_u8(bytes, &mut cursor, "kind")?;
        if !(KIND_RTSP_TCP_C2S..=8).contains(&kind) {
            return Err(DecodeError::InvalidKind(kind));
        }
        let flags = read_u8(bytes, &mut cursor, "flags")?;
        if flags == 0 || (flags & !KNOWN_FLAGS) != 0 {
            return Err(DecodeError::InvalidFlags(flags));
        }

        let _flow_id = read_u16(bytes, &mut cursor, "flow_id")?;
        let _delta_us = read_u32(bytes, &mut cursor, "delta_us")?;
        let payload_len = read_u32(bytes, &mut cursor, "payload_len")? as usize;
        if payload_len == 0 {
            return Err(DecodeError::ZeroLengthPayload(index));
        }
        let payload = read_bytes(bytes, &mut cursor, payload_len, "payload")?.to_vec();
        out.push(RawRecord {
            kind,
            flags,
            payload,
        });
    }

    if cursor != bytes.len() {
        return Err(DecodeError::Truncated("trailing_bytes"));
    }
    Ok(out)
}

fn read_u8(bytes: &[u8], cursor: &mut usize, ctx: &'static str) -> Result<u8, DecodeError> {
    if *cursor + 1 > bytes.len() {
        return Err(DecodeError::Truncated(ctx));
    }
    let value = bytes[*cursor];
    *cursor += 1;
    Ok(value)
}

fn read_u16(bytes: &[u8], cursor: &mut usize, ctx: &'static str) -> Result<u16, DecodeError> {
    let raw = read_bytes(bytes, cursor, 2, ctx)?;
    Ok(u16::from_be_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], cursor: &mut usize, ctx: &'static str) -> Result<u32, DecodeError> {
    let raw = read_bytes(bytes, cursor, 4, ctx)?;
    Ok(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_bytes<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    len: usize,
    ctx: &'static str,
) -> Result<&'a [u8], DecodeError> {
    if *cursor + len > bytes.len() {
        return Err(DecodeError::Truncated(ctx));
    }
    let slice = &bytes[*cursor..*cursor + len];
    *cursor += len;
    Ok(slice)
}

#[test]
fn decode_helper_rejects_invalid_magic_and_flags() {
    let mut sample = vec![];
    sample.extend_from_slice(b"NOPE");
    sample.extend_from_slice(&1u32.to_be_bytes());
    sample.push(KIND_RTSP_TCP_S2C);
    sample.push(FLAG_STANDARD_ASSERTABLE);
    sample.extend_from_slice(&1u16.to_be_bytes());
    sample.extend_from_slice(&0u32.to_be_bytes());
    sample.extend_from_slice(&1u32.to_be_bytes());
    sample.push(b'x');
    assert!(matches!(
        decode_rtspcap(&sample),
        Err(DecodeError::BadMagic)
    ));

    sample[0..4].copy_from_slice(b"RSF1");
    sample[9] = 0;
    assert!(matches!(
        decode_rtspcap(&sample),
        Err(DecodeError::InvalidFlags(0))
    ));
}

#[test]
fn interleaved_frames_emit_events_and_payloads_parse() {
    let records = decode_rtspcap(H264_TCP_PUBLISH_PLAY).expect("fixture decode should succeed");
    let interleaved_rtp = payloads_by_kind(&records, KIND_TCP_INTERLEAVED_RTP);

    assert!(
        !interleaved_rtp.is_empty(),
        "standard tcp fixture should include interleaved RTP payloads"
    );

    let mut core = RtspCore::new();

    let mut rtp_ok = 0usize;
    for payload in interleaved_rtp.iter().take(64) {
        let framed = encode_interleaved_frame(0, payload);
        let outputs = core
            .handle_input(CoreInput::Bytes(framed))
            .expect("interleaved frame feed should not fail");
        assert_interleaved_event(&outputs, 0, payload);

        if let Ok(packet) = RtpPacket::parse(payload) {
            assert_eq!(packet.header.version, 2);
            assert!(
                !packet.payload.is_empty(),
                "interleaved RTP payload should not be empty after parsing"
            );
            let _ = packet.header.sequence_number;
            let _ = packet.header.timestamp;
            rtp_ok += 1;
        }
    }
    assert!(
        rtp_ok > 0,
        "at least one interleaved RTP payload should parse"
    );

    let synthetic_rtcp = sample_rtcp_sr_packet();
    let framed = encode_interleaved_frame(1, &synthetic_rtcp);
    let outputs = core
        .handle_input(CoreInput::Bytes(framed))
        .expect("synthetic interleaved RTCP frame feed should not fail");
    assert_interleaved_event(&outputs, 1, &synthetic_rtcp);
    let parsed = RtcpPacket::parse(&synthetic_rtcp).expect("synthetic RTCP should parse");
    assert!(
        !parsed.is_empty(),
        "synthetic RTCP parse result should not be empty"
    );
}

#[test]
fn udp_rtp_rtcp_payloads_are_parseable() {
    let records = decode_rtspcap(H264_UDP_PUBLISH_PLAY).expect("fixture decode should succeed");
    let udp_rtp = payloads_by_kinds(&records, &[KIND_UDP_PUBLISH_RTP, KIND_UDP_PLAY_RTP]);

    assert!(
        !udp_rtp.is_empty(),
        "standard udp fixture should include RTP datagrams"
    );

    let mut parsed_rtp = 0usize;
    for datagram in udp_rtp.iter().take(128) {
        if let Ok(packet) = RtpPacket::parse(datagram) {
            assert_eq!(packet.header.version, 2);
            parsed_rtp += 1;
        }
    }
    assert!(parsed_rtp > 0, "at least one UDP RTP datagram should parse");

    let parsed_rtcp =
        RtcpPacket::parse(&sample_rtcp_sr_packet()).expect("synthetic UDP RTCP should parse");
    assert!(
        !parsed_rtcp.is_empty(),
        "synthetic UDP RTCP parse result should not be empty"
    );
}

#[test]
fn standard_and_probe_fault_views_are_bounded_and_no_panic() {
    let fixtures = [
        CaptureFixture {
            case: "h264_tcp_publish_play",
            bytes: H264_TCP_PUBLISH_PLAY,
        },
        CaptureFixture {
            case: "h264_udp_publish_play",
            bytes: H264_UDP_PUBLISH_PLAY,
        },
        CaptureFixture {
            case: "h265_tcp_publish_play",
            bytes: H265_TCP_PUBLISH_PLAY,
        },
        CaptureFixture {
            case: "audio_only_udp_publish_play",
            bytes: AUDIO_ONLY_UDP_PUBLISH_PLAY,
        },
        CaptureFixture {
            case: "av1_probe",
            bytes: AV1_PROBE,
        },
        CaptureFixture {
            case: "vp8_probe",
            bytes: VP8_PROBE,
        },
        CaptureFixture {
            case: "vp9_probe",
            bytes: VP9_PROBE,
        },
        CaptureFixture {
            case: "h266_probe",
            bytes: H266_PROBE,
        },
        CaptureFixture {
            case: "high_bitrate_probe",
            bytes: HIGH_BITRATE_PROBE,
        },
    ];

    for fixture in fixtures {
        let records = decode_rtspcap(fixture.bytes).expect("fixture decode should succeed");
        let mut tcp_payloads = payloads_by_kind(&records, KIND_RTSP_TCP_C2S);
        if tcp_payloads.len() > MAX_ROBUSTNESS_RECORDS {
            tcp_payloads.truncate(MAX_ROBUSTNESS_RECORDS);
        }
        if tcp_payloads.is_empty() {
            continue;
        }

        for (view_name, payloads) in tcp_fault_views(&tcp_payloads) {
            let mut core = RtspCore::new();
            let mut seen_outputs = 0usize;
            for payload in payloads {
                match core.handle_input(CoreInput::Bytes(Bytes::from(payload))) {
                    Ok(outputs) => {
                        seen_outputs = seen_outputs.saturating_add(outputs.len());
                        assert!(
                            seen_outputs <= MAX_VIEW_EVENTS,
                            "fixture {} view {} exceeds bounded event limit",
                            fixture.case,
                            view_name
                        );
                    }
                    Err(_) => {
                        // fault views are allowed to fail parsing; bounded termination is the requirement.
                        break;
                    }
                }
            }
        }
    }
}

fn assert_interleaved_event(outputs: &[CoreOutput], expected_channel: u8, expected_payload: &[u8]) {
    let mut matched = false;
    for output in outputs {
        if let CoreOutput::Event(RtspEvent::InterleavedFrame { channel, payload }) = output {
            if *channel == expected_channel && payload.as_ref() == expected_payload {
                matched = true;
                break;
            }
        }
    }
    assert!(
        matched,
        "expected interleaved event channel={} payload_len={}",
        expected_channel,
        expected_payload.len()
    );
}

fn encode_interleaved_frame(channel: u8, payload: &[u8]) -> Bytes {
    let len: u16 = payload
        .len()
        .try_into()
        .expect("interleaved payload length should fit u16");
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.push(b'$');
    framed.push(channel);
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(payload);
    Bytes::from(framed)
}

fn tcp_fault_views(payloads: &[Vec<u8>]) -> Vec<(&'static str, Vec<Vec<u8>>)> {
    vec![
        ("coalesced_pairs", coalesced_pairs(payloads)),
        ("prefix_truncated_half", prefix_truncated(payloads, 1, 2)),
        (
            "prefix_truncated_three_quarters",
            prefix_truncated(payloads, 3, 4),
        ),
        ("suffix_truncated_record", suffix_truncated_record(payloads)),
        ("duplicated_record", duplicated_record(payloads)),
        ("reordered_adjacent", reordered_adjacent(payloads)),
        ("dropped_every_5th", dropped_every_nth(payloads, 5)),
    ]
}

fn coalesced_pairs(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for chunk in payloads.chunks(2) {
        let mut merged = Vec::new();
        for payload in chunk {
            merged.extend_from_slice(payload);
        }
        out.push(merged);
    }
    out
}

fn prefix_truncated(payloads: &[Vec<u8>], numer: usize, denom: usize) -> Vec<Vec<u8>> {
    let total_bytes: usize = payloads.iter().map(Vec::len).sum();
    let mut budget = total_bytes.saturating_mul(numer) / denom.max(1);
    budget = budget.max(1);

    let mut out = Vec::new();
    for payload in payloads {
        if budget == 0 {
            break;
        }
        let take = budget.min(payload.len());
        if take > 0 {
            out.push(payload[..take].to_vec());
            budget -= take;
        }
    }
    if out.is_empty() && !payloads.is_empty() {
        out.push(payloads[0][..1.min(payloads[0].len())].to_vec());
    }
    out
}

fn suffix_truncated_record(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = payloads.to_vec();
    if let Some(last) = out.last_mut() {
        let keep = (last.len() / 2).max(1);
        last.truncate(keep);
    }
    out
}

fn duplicated_record(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = payloads.to_vec();
    if out.is_empty() {
        return out;
    }
    let idx = if out.len() > 1 { 1 } else { 0 };
    out.insert(idx + 1, out[idx].clone());
    out
}

fn reordered_adjacent(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = payloads.to_vec();
    if out.len() >= 3 {
        out.swap(1, 2);
    } else if out.len() >= 2 {
        out.swap(0, 1);
    }
    out
}

fn dropped_every_nth(payloads: &[Vec<u8>], n: usize) -> Vec<Vec<u8>> {
    if n < 2 {
        return payloads.to_vec();
    }
    let mut out = Vec::new();
    for (idx, payload) in payloads.iter().enumerate() {
        if (idx + 1) % n != 0 {
            out.push(payload.clone());
        }
    }
    if out.is_empty() && !payloads.is_empty() {
        out.push(payloads[0].clone());
    }
    out
}

fn payloads_by_kind(records: &[RawRecord], kind: u8) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| record.kind == kind)
        .map(|record| record.payload.clone())
        .collect()
}

fn payloads_by_kinds(records: &[RawRecord], kinds: &[u8]) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| kinds.contains(&record.kind))
        .map(|record| record.payload.clone())
        .collect()
}

fn sample_rtcp_sr_packet() -> Vec<u8> {
    let mut packet = Vec::with_capacity(28);
    packet.push(0x80); // V=2, P=0, RC=0
    packet.push(200); // SR
    packet.extend_from_slice(&6u16.to_be_bytes()); // 24-byte payload => 6 words
    packet.extend_from_slice(&0x1122_3344u32.to_be_bytes()); // ssrc
    packet.extend_from_slice(&0u64.to_be_bytes()); // ntp ts
    packet.extend_from_slice(&0x0102_0304u32.to_be_bytes()); // rtp ts
    packet.extend_from_slice(&1u32.to_be_bytes()); // packet count
    packet.extend_from_slice(&160u32.to_be_bytes()); // octet count
    packet
}
