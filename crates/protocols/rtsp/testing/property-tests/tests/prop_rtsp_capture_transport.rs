#[allow(dead_code)]
#[path = "support/rtsp_capture_fixture.rs"]
mod rtsp_capture_fixture;

use std::path::Path;
use std::sync::OnceLock;

use bytes::Bytes;
use cheetah_rtsp_core::{
    CoreInput, CoreOutput, RtcpPacket, RtpPacket, RtspCore, RtspMethod, RtspRequest, RtspTransport,
    Sdp,
};
use proptest::prelude::*;
use rtsp_capture_fixture::{
    build_tcp_fault_views, build_udp_rtp_fault_views, load_capture_fixtures, CaptureFixture,
    CaptureRecord, CaptureRecordKind, CaptureRole,
};

const MANIFEST: &str = include_str!("testdata/rtsp-capture/manifest.tsv");
const MAX_TCP_RECORDS: usize = 256;
const MAX_UDP_DATAGRAMS: usize = 256;
const MAX_EVENTS: usize = 16_384;

fn fixture_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/rtsp-capture")
}

fn fixtures() -> &'static [CaptureFixture] {
    static FIXTURES: OnceLock<Vec<CaptureFixture>> = OnceLock::new();
    FIXTURES
        .get_or_init(|| {
            load_capture_fixtures(&fixture_root(), MANIFEST)
                .expect("committed capture fixtures should be valid")
        })
        .as_slice()
}

fn is_standard_role(role: CaptureRole) -> bool {
    matches!(
        role,
        CaptureRole::StandardPublishTcp
            | CaptureRole::StandardPublishUdp
            | CaptureRole::StandardPublishHttpTunnel
            | CaptureRole::StandardPlayTcp
            | CaptureRole::StandardPlayUdp
            | CaptureRole::StandardPlayMulticast
            | CaptureRole::StandardPullJob
            | CaptureRole::StandardPushJob
            | CaptureRole::StandardRelayJob
    )
}

fn tcp_c2s_payloads(records: &[CaptureRecord]) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| record.kind == CaptureRecordKind::RtspTcpC2s)
        .map(|record| record.payload.clone())
        .take(MAX_TCP_RECORDS)
        .collect()
}

fn replay_requests(
    chunks: &[Vec<u8>],
) -> Result<Vec<RtspRequest>, cheetah_rtsp_core::RtspCoreError> {
    let mut core = RtspCore::new();
    let mut requests = Vec::new();
    let mut total_events = 0usize;

    for chunk in chunks {
        let outputs = core.handle_input(CoreInput::Bytes(Bytes::copy_from_slice(chunk)))?;
        total_events = total_events.saturating_add(outputs.len());
        if total_events > MAX_EVENTS {
            break;
        }
        for output in outputs {
            if let CoreOutput::Event(cheetah_rtsp_core::RtspEvent::Request(req)) = output {
                requests.push(req);
            }
        }
    }
    Ok(requests)
}

fn methods_subsequence(methods: &[RtspMethod], expected: &[RtspMethod]) -> bool {
    if expected.is_empty() {
        return true;
    }
    let mut idx = 0usize;
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

fn header_value<'a>(request: &'a RtspRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

fn assert_standard_request_semantics(fixture: &CaptureFixture, requests: &[RtspRequest]) {
    let methods: Vec<RtspMethod> = requests.iter().map(|req| req.method.clone()).collect();
    assert!(
        methods.contains(&RtspMethod::Options),
        "fixture {} should include OPTIONS in unperturbed view",
        fixture.row.case
    );

    // Full publish+play sequence is only guaranteed by the current committed H264 standard fixtures.
    if fixture.row.case == "h264_tcp_publish_play" || fixture.row.case == "h264_udp_publish_play" {
        assert!(
            methods_subsequence(
                &methods,
                &[
                    RtspMethod::Options,
                    RtspMethod::Announce,
                    RtspMethod::Setup,
                    RtspMethod::Record
                ]
            ),
            "fixture {} should include publish method sequence",
            fixture.row.case
        );
        assert!(
            methods_subsequence(
                &methods,
                &[
                    RtspMethod::Options,
                    RtspMethod::Describe,
                    RtspMethod::Setup,
                    RtspMethod::Play
                ]
            ),
            "fixture {} should include play method sequence",
            fixture.row.case
        );
    }

    let mut setup_seen = false;
    for request in requests {
        assert!(
            request.cseq.is_some(),
            "fixture {} request {} must have parseable CSeq",
            fixture.row.case,
            request.method
        );

        if request.method == RtspMethod::Announce {
            assert!(
                !request.body.is_empty(),
                "ANNOUNCE body should not be empty"
            );
            let body = std::str::from_utf8(&request.body).expect("ANNOUNCE SDP should be utf-8");
            Sdp::parse(body).expect("ANNOUNCE SDP should parse");
        }

        if request.method == RtspMethod::Setup {
            setup_seen = true;
            let transport =
                header_value(request, "Transport").expect("SETUP must include Transport header");
            let parsed = RtspTransport::parse_multiple(transport).expect("SETUP transport parse");
            assert!(
                !parsed.is_empty(),
                "SETUP transport parse should be non-empty"
            );
        }
    }

    if fixture.row.case == "h264_tcp_publish_play" || fixture.row.case == "h264_udp_publish_play" {
        assert!(
            setup_seen,
            "fixture {} should include SETUP",
            fixture.row.case
        );
    }
}

fn rechunk(payloads: &[Vec<u8>], chunk_size: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for payload in payloads {
        if payload.len() <= chunk_size {
            out.push(payload.clone());
            continue;
        }
        for chunk in payload.chunks(chunk_size) {
            out.push(chunk.to_vec());
        }
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

fn udp_datagrams(records: &[CaptureRecord]) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                CaptureRecordKind::UdpPublishRtp
                    | CaptureRecordKind::UdpPublishRtcp
                    | CaptureRecordKind::UdpPlayRtp
                    | CaptureRecordKind::UdpPlayRtcp
            )
        })
        .map(|record| record.payload.clone())
        .take(MAX_UDP_DATAGRAMS)
        .collect()
}

fn is_rtcp_payload(payload: &[u8]) -> bool {
    payload.len() >= 2 && (payload[0] >> 6) == 2 && (200..=204).contains(&payload[1])
}

fn is_monotonic_non_decreasing(sequence: &[u16]) -> bool {
    sequence.windows(2).all(|pair| pair[0] <= pair[1])
}

fn view_by_name<'a>(
    views: &'a [rtsp_capture_fixture::NamedPayloadView],
    name: &str,
) -> Option<&'a [Vec<u8>]> {
    views
        .iter()
        .find(|view| view.name == name)
        .map(|view| view.payloads.as_slice())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn prop_rtsp_capture_tcp_transport(
        fixture_index in 0usize..64,
        mode in 0u8..9,
        coalesced_n in 2usize..8,
        drop_every_nth in 2usize..8,
        chunk_size in 1usize..24,
    ) {
        let all = fixtures();
        let fixture = &all[fixture_index % all.len()];
        let payloads = tcp_c2s_payloads(&fixture.records);
        prop_assume!(!payloads.is_empty());

        let fault_views = build_tcp_fault_views(&fixture.records, coalesced_n, drop_every_nth)
            .expect("c2s payloads should build tcp fault views");

        let mut selected = match mode {
            0 => payloads.clone(),
            1 => view_by_name(&fault_views, "tcp_single_buffer").unwrap_or(&payloads).to_vec(),
            2 => view_by_name(&fault_views, "tcp_one_byte_chunks").unwrap_or(&payloads).to_vec(),
            3 => view_by_name(&fault_views, "tcp_coalesced_n").unwrap_or(&payloads).to_vec(),
            4 => view_by_name(&fault_views, "tcp_prefix_truncated_half").unwrap_or(&payloads).to_vec(),
            5 => suffix_truncated_record(&payloads),
            6 => view_by_name(&fault_views, "tcp_duplicate_record").unwrap_or(&payloads).to_vec(),
            7 => view_by_name(&fault_views, "tcp_swap_adjacent").unwrap_or(&payloads).to_vec(),
            _ => view_by_name(&fault_views, "tcp_drop_every_nth").unwrap_or(&payloads).to_vec(),
        };

        if mode != 0 && mode != 2 {
            selected = rechunk(&selected, chunk_size);
        }

        let result = replay_requests(&selected);
        if is_standard_role(fixture.row.role) && mode == 0 {
            let requests = result.expect("standard original tcp replay should succeed");
            prop_assert!(!requests.is_empty());
            assert_standard_request_semantics(fixture, &requests);
        } else if let Ok(requests) = result {
            prop_assert!(requests.len() <= MAX_EVENTS);
        }
    }

    #[test]
    fn prop_rtsp_capture_udp_transport(
        fixture_index in 0usize..64,
        mode in 0u8..7,
        drop_every_nth in 2usize..8,
        reverse_window in 2usize..8,
    ) {
        let all = fixtures();
        let fixture = &all[fixture_index % all.len()];
        let original = udp_datagrams(&fixture.records);
        prop_assume!(!original.is_empty());

        let fault_views = build_udp_rtp_fault_views(&fixture.records, drop_every_nth, reverse_window)
            .expect("udp records should build udp fault views");

        let selected = match mode {
            0 => original.clone(),
            1 => view_by_name(&fault_views, "udp_drop_datagram").unwrap_or(&original).to_vec(),
            2 => view_by_name(&fault_views, "udp_duplicate_datagram").unwrap_or(&original).to_vec(),
            3 => view_by_name(&fault_views, "udp_swap_adjacent_datagrams").unwrap_or(&original).to_vec(),
            4 => view_by_name(&fault_views, "udp_reverse_small_window").unwrap_or(&original).to_vec(),
            5 => view_by_name(&fault_views, "udp_truncate_payload").unwrap_or(&original).to_vec(),
            _ => view_by_name(&fault_views, "rtp_sequence_reorder").unwrap_or(&original).to_vec(),
        };

        let mut rtp_sequences: std::collections::BTreeMap<u32, Vec<u16>> = std::collections::BTreeMap::new();
        for datagram in selected.iter().take(MAX_UDP_DATAGRAMS) {
            if is_rtcp_payload(datagram) {
                let _ = RtcpPacket::parse(datagram);
                continue;
            }
            if let Ok(packet) = RtpPacket::parse(datagram) {
                rtp_sequences
                    .entry(packet.header.ssrc)
                    .or_default()
                    .push(packet.header.sequence_number);
            }
        }

        if is_standard_role(fixture.row.role) && mode == 0 {
            let mut checked = 0usize;
            for seq in rtp_sequences.values() {
                if seq.len() >= 2 {
                    prop_assert!(is_monotonic_non_decreasing(seq));
                    checked += 1;
                }
            }
            prop_assert!(checked > 0);
        } else {
            prop_assert!(rtp_sequences.len() <= MAX_UDP_DATAGRAMS);
        }
    }
}
