//! Property-based transport tests using committed RTSP capture fixtures.
//!
//! These tests replay captured TCP C2S and UDP RTP/RTCP payloads under various
//! fault views (single buffer, one-byte chunks, coalesced, truncated, dropped,
//! swapped, reordered) and assert that the `RtspCore` parser remains robust.
//!
//! RTSP 抓包 fixture 的属性传输测试。
//!
//! 这些测试在不同故障视图下（单缓冲、逐字节、合并、截断、丢弃、交换、重排）
//! 回放已捕获的 TCP C2S 与 UDP RTP/RTCP payload，并验证 `RtspCore` 解析器的鲁棒性。

#[allow(dead_code)]
#[path = "support/rtsp_capture_fixture.rs"]
mod rtsp_capture_fixture;

use std::path::Path;
use std::sync::OnceLock;

use bytes::Bytes;
use cheetah_rtsp_core::{
    CoreInput, CoreOutput, RtcpPacket, RtpPacket, RtspCore, RtspEvent, RtspMethod, RtspRequest,
    RtspTransport,
};
use proptest::prelude::*;
use rtsp_capture_fixture::{
    build_tcp_fault_views, build_udp_rtp_fault_views, load_capture_fixtures, CaptureFixture,
    CaptureRecord, CaptureRecordKind, CaptureRole, NamedPayloadView,
};

/// Embedded manifest content for the committed fixtures.
///
/// 已提交 fixture 的内嵌清单内容。
const MANIFEST: &str = include_str!("testdata/rtsp-capture/manifest.tsv");

/// Upper bound on the number of TCP records to replay.
///
/// TCP 记录回放数量上限。
const MAX_TCP_RECORDS: usize = 256;

/// Upper bound on the number of UDP datagrams to inspect.
///
/// UDP 数据包检查数量上限。
const MAX_UDP_DATAGRAMS: usize = 256;

/// Upper bound on the number of core events produced during replay.
///
/// 回放过程中 core 事件数量上限。
const MAX_EVENTS: usize = 16_384;

/// Resolve the fixture root directory relative to the crate manifest.
///
/// 解析 crate 清单相对的 fixture 根目录。
fn fixture_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/rtsp-capture")
}

/// Lazily load all committed capture fixtures.
///
/// 延迟加载所有已提交的抓包 fixture。
fn fixtures() -> &'static [CaptureFixture] {
    static FIXTURES: OnceLock<Vec<CaptureFixture>> = OnceLock::new();
    FIXTURES
        .get_or_init(|| {
            load_capture_fixtures(&fixture_root(), MANIFEST)
                .expect("committed capture fixtures should be valid")
        })
        .as_slice()
}

/// Check whether a fixture role is a standard, supported role.
///
/// 检查 fixture 角色是否为受支持的标准角色。
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

/// Extract TCP client-to-server payloads from the records.
///
/// 从记录中提取 TCP 客户端到服务端 payload。
fn tcp_c2s_payloads(records: &[CaptureRecord]) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| record.kind == CaptureRecordKind::RtspTcpC2s)
        .map(|record| record.payload.clone())
        .take(MAX_TCP_RECORDS)
        .collect()
}

/// Replay byte chunks through a fresh `RtspCore` and collect all parsed requests.
///
/// 通过全新的 `RtspCore` 回放字节块并收集解析出的请求。
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
            if let CoreOutput::Event(RtspEvent::Request(req)) = output {
                requests.push(req);
            }
        }
    }
    Ok(requests)
}

/// Check whether `expected` is a subsequence of `methods` (order-preserving).
///
/// 检查 `expected` 是否为 `methods` 的保持顺序子序列。
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

/// Return the value of a request header by name (case-insensitive).
///
/// 按名称（大小写不敏感）返回请求头值。
fn header_value<'a>(request: &'a RtspRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

/// Assert that the replayed requests contain the expected RTSP method sequences
/// and well-formed metadata for the given fixture.
///
/// 断言回放的请求包含预期的 RTSP 方法序列与给定 fixture 的格式良好元数据。
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
            cheetah_rtsp_core::Sdp::parse(body).expect("ANNOUNCE SDP should parse");
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

/// Re-chunk each payload into pieces of at most `chunk_size` bytes.
///
/// 将每个 payload 重新切分为不超过 `chunk_size` 字节的片段。
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

/// Truncate the final payload to roughly half its length.
///
/// 将最后一个 payload 截断到约一半长度。
fn suffix_truncated_record(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = payloads.to_vec();
    if let Some(last) = out.last_mut() {
        let keep = (last.len() / 2).max(1);
        last.truncate(keep);
    }
    out
}

/// Extract UDP RTP/RTCP datagram payloads from the records.
///
/// 从记录中提取 UDP RTP/RTCP 数据包 payload。
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

/// Check whether a payload is an RTCP packet based on the version and packet type.
///
/// 根据版本与包类型判断 payload 是否为 RTCP 包。
fn is_rtcp_payload(payload: &[u8]) -> bool {
    payload.len() >= 2 && (payload[0] >> 6) == 2 && (200..=204).contains(&payload[1])
}

/// Check whether a sequence of u16 values is monotonic non-decreasing.
///
/// 检查 u16 序列是否单调非递减。
fn is_monotonic_non_decreasing(sequence: &[u16]) -> bool {
    sequence.windows(2).all(|pair| pair[0] <= pair[1])
}

/// Find a fault view by name and return its payload slice.
///
/// 按名称查找故障视图并返回其 payload 切片。
fn view_by_name<'a>(views: &'a [NamedPayloadView], name: &str) -> Option<&'a [Vec<u8>]> {
    views
        .iter()
        .find(|view| view.name == name)
        .map(|view| view.payloads.as_slice())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Replay TCP C2S payloads under a random fault view and assert the parser is robust.
    ///
    /// 在随机 TCP 故障视图下回放 C2S payload，并验证解析器鲁棒性。
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

    /// Inspect UDP RTP/RTCP datagrams under a random fault view and assert monotonicity.
    ///
    /// 在随机 UDP 故障视图下检查 RTP/RTCP 数据包，并验证序列单调性。
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
