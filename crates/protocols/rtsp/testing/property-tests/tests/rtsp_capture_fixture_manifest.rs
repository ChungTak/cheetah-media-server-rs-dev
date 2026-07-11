//! Regression tests for RTSP capture fixture manifest and binary format validation.
//!
//! These tests assert that the committed manifest is well-formed, the custom
//! `rtspcap` binary format is decoded correctly, and the fault-view generators
//! cover the required transport modes.
//!
//! RTSP 抓包 fixture 清单与二进制格式验证回归测试。
//!
//! 这些测试断言已提交清单格式正确、自定义 `rtspcap` 二进制格式正确解码，
//! 以及故障视图生成器覆盖所需传输模式。

#[allow(dead_code)]
#[path = "support/rtsp_capture_fixture.rs"]
mod rtsp_capture_fixture;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use cheetah_rtsp_core::{CoreInput, CoreOutput, RtspCore, RtspEvent, RtspMethod, RtspRequest};
use rtsp_capture_fixture::{
    build_tcp_fault_views, build_transport_fault_views, build_udp_rtp_fault_views, decode_rtspcap,
    load_capture_fixtures, parse_manifest, validate_manifest, CaptureFaultViewError,
    CaptureFixtureError, CaptureRecord, CaptureRecordKind, CaptureRole, MANIFEST_HEADER,
    MAX_FIXTURE_BYTES,
};

/// Embedded manifest content.
///
/// 内嵌清单内容。
const MANIFEST: &str = include_str!("testdata/rtsp-capture/manifest.tsv");

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

/// The committed manifest header must match the expected schema and fixture counts.
///
/// 已提交清单表头必须匹配预期模式与 fixture 数量。
#[test]
fn committed_manifest_header_is_valid() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/rtsp-capture");
    let rows = validate_manifest(&root, MANIFEST).expect("committed manifest should be valid");
    assert_eq!(
        rows.len(),
        20,
        "phase 5.1 commits eleven standard and nine probe fixtures"
    );

    let standard_rows = rows.iter().filter(|row| is_standard_role(row.role)).count();
    let probe_rows = rows
        .iter()
        .filter(|row| row.role == CaptureRole::CompatProbe)
        .count();
    assert_eq!(standard_rows, 11);
    assert_eq!(probe_rows, 9);

    for row in rows {
        assert!(
            !row.expect_methods.is_empty(),
            "expect_methods should not be empty"
        );
        if is_standard_role(row.role) {
            assert!(
                row.fixture.starts_with("standard"),
                "standard case should live under standard/"
            );
            if !is_audio_only_media_sig(&row.media_sig) {
                assert!(
                    row.expect_rtp_min >= 1,
                    "non-audio-only standard case should require minimal RTP parsing"
                );
                assert!(
                    row.expect_tracks_min >= 1,
                    "non-audio-only standard case should require at least one track"
                );
            }
        } else if row.role == CaptureRole::CompatProbe {
            assert!(
                row.fixture.starts_with("probes"),
                "probe case should live under probes/"
            );
            assert_eq!(row.expect_rtp_min, 0);
            assert_eq!(row.expect_tracks_min, 0);
        }
    }
}

/// An environment-generated manifest, if present, must also be valid.
///
/// 若存在环境生成的清单，也必须有效。
#[test]
fn generated_manifest_from_env_is_valid() {
    let Ok(root) = std::env::var("RTSP_CAPTURE_FIXTURE_DIR") else {
        return;
    };
    let root = Path::new(&root);
    let manifest = std::fs::read_to_string(root.join("manifest.tsv"))
        .expect("generated manifest should be readable");
    let rows = validate_manifest(root, &manifest).expect("generated manifest should be valid");

    assert!(
        !rows.is_empty(),
        "generated manifest should include at least one fixture row"
    );
}

/// `rtspcap` records with valid fields must decode correctly.
///
/// 字段有效的 `rtspcap` 记录必须正确解码。
#[test]
fn decode_rtspcap_accepts_valid_records() {
    let bytes = build_rtspcap(&[(1, 1, 11, 10, b"A"), (7, 5, 12, 30, b"BC")]);
    let records = decode_rtspcap(&bytes).expect("valid rtspcap should decode");

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].flow_id, 11);
    assert_eq!(records[0].payload, b"A");
    assert_eq!(records[1].flow_id, 12);
    assert_eq!(records[1].payload, b"BC");
}

/// Bad `rtspcap` magic is rejected.
///
/// 错误的 `rtspcap` 魔数被拒绝。
#[test]
fn decode_rtspcap_rejects_bad_magic() {
    let mut bytes = build_rtspcap(&[(1, 1, 1, 0, b"payload")]);
    bytes[0..4].copy_from_slice(b"NOPE");

    assert!(matches!(
        decode_rtspcap(&bytes),
        Err(CaptureFixtureError::BadMagic)
    ));
}

/// Truncated payload bytes are rejected.
///
/// 截断的 payload 字节被拒绝。
#[test]
fn decode_rtspcap_rejects_truncated_payload() {
    let mut bytes = build_rtspcap(&[(1, 1, 1, 0, b"payload")]);
    bytes.pop();

    assert!(matches!(
        decode_rtspcap(&bytes),
        Err(CaptureFixtureError::Truncated { .. })
    ));
}

/// Zero-length records are rejected.
///
/// 零长度记录被拒绝。
#[test]
fn decode_rtspcap_rejects_zero_length_record() {
    let bytes = build_rtspcap(&[(1, 1, 1, 0, b"")]);

    assert!(matches!(
        decode_rtspcap(&bytes),
        Err(CaptureFixtureError::ZeroLengthRecord { index: 0 })
    ));
}

/// Unknown record kinds are rejected.
///
/// 未知记录类型被拒绝。
#[test]
fn decode_rtspcap_rejects_invalid_record_kind() {
    let bytes = build_rtspcap(&[(9, 1, 1, 0, b"payload")]);

    assert!(matches!(
        decode_rtspcap(&bytes),
        Err(CaptureFixtureError::InvalidRecordKind { raw: 9 })
    ));
}

/// Trailing bytes after the declared record count are rejected.
///
/// 声明记录数后的尾随字节被拒绝。
#[test]
fn decode_rtspcap_rejects_trailing_bytes() {
    let mut bytes = build_rtspcap(&[(1, 1, 1, 0, b"payload")]);
    bytes.push(0);

    assert!(matches!(
        decode_rtspcap(&bytes),
        Err(CaptureFixtureError::TrailingBytes { bytes: 1 })
    ));
}

/// Invalid record flags are rejected.
///
/// 无效记录标志被拒绝。
#[test]
fn decode_rtspcap_rejects_invalid_record_flags() {
    let bytes = build_rtspcap(&[(1, 0, 1, 0, b"payload")]);

    assert!(matches!(
        decode_rtspcap(&bytes),
        Err(CaptureFixtureError::InvalidRecordFlags { raw: 0 })
    ));
}

/// A manifest without the expected header is rejected.
///
/// 没有预期表头的清单被拒绝。
#[test]
fn manifest_rejects_bad_header() {
    let err = parse_manifest("bad\theader\n").expect_err("bad header should fail");

    assert!(matches!(
        err,
        CaptureFixtureError::InvalidManifestHeader { .. }
    ));
}

/// A manifest row with the wrong number of fields is rejected.
///
/// 字段数错误的清单行被拒绝。
#[test]
fn manifest_rejects_bad_field_count() {
    let input = format!("{MANIFEST_HEADER}\ncase\ttoo-short\n");
    let err = parse_manifest(&input).expect_err("short row should fail");

    assert!(matches!(
        err,
        CaptureFixtureError::InvalidManifestFieldCount {
            line: 2,
            expected: 13,
            actual: 2
        }
    ));
}

/// An unknown role string is rejected.
///
/// 未知角色字符串被拒绝。
#[test]
fn manifest_rejects_invalid_role() {
    let input = format!(
        "{MANIFEST_HEADER}\ncase\tcapture.pcap\tstream\tv=h264@1x1;a=aac@ch2\ttcp\ttcp\tunknown\tstandard/case.rtspcap\tOPTIONS,ANNOUNCE\t1\t0\t1\tnote\n"
    );
    let err = parse_manifest(&input).expect_err("invalid role should fail");

    assert!(matches!(
        err,
        CaptureFixtureError::InvalidRole { line: 2, .. }
    ));
}

/// An unknown transport string is rejected.
///
/// 未知传输字符串被拒绝。
#[test]
fn manifest_rejects_invalid_transport() {
    let input = format!(
        "{MANIFEST_HEADER}\ncase\tcapture.pcap\tstream\tv=h264@1x1;a=aac@ch2\tbogus\ttcp\tcompat_probe\tprobes/case.rtspcap\tOPTIONS\t0\t0\t0\tnote\n"
    );
    let err = parse_manifest(&input).expect_err("invalid transport should fail");

    assert!(matches!(
        err,
        CaptureFixtureError::InvalidTransport {
            line: 2,
            field: "push_transport",
            ..
        }
    ));
}

/// Fixture paths outside the manifest root are rejected.
///
/// 清单根目录外的 fixture 路径被拒绝。
#[test]
fn manifest_rejects_unsafe_fixture_path() {
    let input = format!(
        "{MANIFEST_HEADER}\ncase\tcapture.pcap\tstream\tv=h264@1x1;a=aac@ch2\ttcp\ttcp\tcompat_probe\t../case.rtspcap\tOPTIONS\t0\t0\t0\tnote\n"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/rtsp-capture");
    let err = validate_manifest(&root, &input).expect_err("unsafe path should fail");

    assert!(matches!(err, CaptureFixtureError::UnsafeFixturePath { .. }));
}

/// Extended transport and role enumerations are accepted.
///
/// 扩展传输与角色枚举被接受。
#[test]
fn manifest_accepts_extended_transport_and_role_enums() {
    let input = format!(
        "{MANIFEST_HEADER}\n\
case-a\tcapture-a.pcap\tstream-a\tv=h264@1x1;a=aac@ch2\thttp-tunnel\thttp-tunnel\tstandard_publish_http_tunnel\tstandard/a.rtspcap\tOPTIONS\t1\t0\t1\tnote\n\
case-b\tcapture-b.pcap\tstream-b\tv=h264@1x1;a=aac@ch2\tudp\tmulticast\tstandard_play_multicast\tstandard/b.rtspcap\tOPTIONS\t1\t1\t1\tnote\n\
case-c\tcapture-c.pcap\tstream-c\tv=h264@1x1;a=aac@ch2\tmixed\ttcp\tstandard_push_job\tstandard/c.rtspcap\tOPTIONS\t1\t0\t1\tnote\n\
case-d\tcapture-d.pcap\tstream-d\tv=h264@1x1;a=aac@ch2\ttcp\tmixed\tstandard_pull_job\tstandard/d.rtspcap\tOPTIONS\t1\t0\t1\tnote\n\
case-e\tcapture-e.pcap\tstream-e\tv=h264@1x1;a=aac@ch2\tmixed\tmixed\tstandard_relay_job\tstandard/e.rtspcap\tOPTIONS\t1\t0\t1\tnote\n\
case-f\tcapture-f.pcap\tstream-f\tv=h264@1x1;a=aac@ch2\tnone\tnone\ttransport_fault_seed\tprobes/f.rtspcap\tOPTIONS\t0\t0\t0\tnote\n"
    );

    let rows = parse_manifest(&input).expect("extended transport/role enums should parse");
    assert_eq!(rows.len(), 6);
}

/// A referenced fixture missing from disk is rejected.
///
/// 磁盘上缺失的引用 fixture 被拒绝。
#[test]
fn manifest_rejects_missing_fixture() {
    let input = format!(
        "{MANIFEST_HEADER}\ncase\tcapture.pcap\tstream\tv=h264@1x1;a=aac@ch2\ttcp\ttcp\tstandard_publish_tcp\tstandard/not-found.rtspcap\tOPTIONS\t1\t0\t1\tnote\n"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/rtsp-capture");
    let err = validate_manifest(&root, &input).expect_err("missing fixture should fail");

    assert!(matches!(
        err,
        CaptureFixtureError::MissingFixture { line: 2, .. }
    ));
}

/// A fixture larger than the allowed size is rejected.
///
/// 超过允许大小的 fixture 被拒绝。
#[test]
fn manifest_rejects_fixture_exceeding_size_limit() {
    let temp_root = create_temp_fixture_root("rtsp_manifest_fixture_too_large");
    let standard_dir = temp_root.join("standard");
    std::fs::create_dir_all(&standard_dir).expect("create standard dir");

    let big_fixture = standard_dir.join("too-large.rtspcap");
    let file = std::fs::File::create(&big_fixture).expect("create oversized fixture");
    file.set_len(MAX_FIXTURE_BYTES + 1)
        .expect("set oversized fixture len");

    let input = format!(
        "{MANIFEST_HEADER}\ncase\tcapture.pcap\tstream\tv=h264@1x1;a=aac@ch2\ttcp\ttcp\tstandard_publish_tcp\tstandard/too-large.rtspcap\tOPTIONS\t1\t0\t1\tnote\n"
    );
    let err = validate_manifest(&temp_root, &input).expect_err("oversized fixture should fail");

    assert!(matches!(
        err,
        CaptureFixtureError::FixtureTooLarge { line: 2, .. }
    ));

    let _ = std::fs::remove_dir_all(&temp_root);
}

/// The fixture matrix covers the required server/publish/play/job roles.
///
/// fixture 矩阵覆盖所需的服务/发布/播放/任务角色。
#[test]
fn fixture_matrix_covers_server_publish_play_and_rtsp_jobs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/rtsp-capture");
    let fixtures = load_capture_fixtures(&root, MANIFEST).expect("fixtures should be loadable");

    let required_roles = [
        CaptureRole::StandardPublishTcp,
        CaptureRole::StandardPublishUdp,
        CaptureRole::StandardPublishHttpTunnel,
        CaptureRole::StandardPlayMulticast,
        CaptureRole::StandardPullJob,
        CaptureRole::StandardPushJob,
        CaptureRole::StandardRelayJob,
    ];
    for role in required_roles {
        assert!(
            fixtures.iter().any(|fixture| fixture.row.role == role),
            "matrix must include at least one fixture for role {role:?}"
        );
    }

    let mut seen_options = false;
    let mut seen_announce = false;
    let mut seen_describe = false;
    let mut seen_setup = false;
    let mut seen_play = false;
    let mut seen_record = false;

    for fixture in fixtures
        .iter()
        .filter(|fixture| is_standard_role(fixture.row.role))
    {
        let requests = decode_requests_from_records(&fixture.records)
            .unwrap_or_else(|err| panic!("decode requests failed for {}: {err}", fixture.row.case));
        for request in &requests {
            match request.method {
                RtspMethod::Options => seen_options = true,
                RtspMethod::Announce => seen_announce = true,
                RtspMethod::Describe => seen_describe = true,
                RtspMethod::Setup => seen_setup = true,
                RtspMethod::Play => seen_play = true,
                RtspMethod::Record => seen_record = true,
                _ => {}
            }
        }

        let (rtp_count, rtcp_count) = count_rtp_rtcp_packets(&fixture.records);
        assert!(
            rtp_count >= fixture.row.expect_rtp_min,
            "fixture {} RTP count {} should be >= expect_rtp_min {}",
            fixture.row.case,
            rtp_count,
            fixture.row.expect_rtp_min
        );
        let _ = rtcp_count;

        let setup_track_controls = unique_setup_track_controls(&requests);
        assert!(
            setup_track_controls.len() >= fixture.row.expect_tracks_min,
            "fixture {} setup track count {} should be >= expect_tracks_min {}",
            fixture.row.case,
            setup_track_controls.len(),
            fixture.row.expect_tracks_min
        );
    }

    assert!(
        seen_options && seen_announce && seen_describe && seen_setup && seen_play && seen_record,
        "standard fixture matrix should cover OPTIONS/ANNOUNCE/DESCRIBE/SETUP/PLAY/RECORD"
    );
}

/// TCP fault views include all required named variants.
///
/// TCP 故障视图包含所有必需的命名变体。
#[test]
fn tcp_fault_views_cover_required_modes() {
    let records = sample_records();
    let views = build_tcp_fault_views(&records, 2, 3).expect("tcp views should build");
    let names: Vec<&str> = views.iter().map(|view| view.name).collect();

    for expected in [
        "tcp_single_buffer",
        "tcp_original_records",
        "tcp_one_byte_chunks",
        "tcp_coalesced_n",
        "tcp_prefix_truncated_half",
        "tcp_duplicate_record",
        "tcp_swap_adjacent",
        "tcp_drop_every_nth",
    ] {
        assert!(
            names.contains(&expected),
            "missing tcp fault view {expected}"
        );
    }
}

/// UDP fault views include all required named variants.
///
/// UDP 故障视图包含所有必需的命名变体。
#[test]
fn udp_fault_views_cover_required_modes() {
    let records = sample_records();
    let views = build_udp_rtp_fault_views(&records, 3, 3).expect("udp views should build");
    let names: Vec<&str> = views.iter().map(|view| view.name).collect();

    for expected in [
        "udp_drop_datagram",
        "udp_duplicate_datagram",
        "udp_swap_adjacent_datagrams",
        "udp_reverse_small_window",
        "udp_truncate_payload",
        "rtp_sequence_reorder",
    ] {
        assert!(
            names.contains(&expected),
            "missing udp fault view {expected}"
        );
    }
}

/// Fault-view generators reject invalid configuration parameters.
///
/// 故障视图生成器拒绝无效配置参数。
#[test]
fn fault_view_rejects_invalid_configuration() {
    let records = sample_records();

    let tcp_err = build_tcp_fault_views(&records, 1, 2).expect_err("coalesced_n=1 should fail");
    assert!(matches!(
        tcp_err,
        CaptureFaultViewError::InvalidConfig {
            view: "tcp_coalesced_n",
            ..
        }
    ));

    let udp_err =
        build_udp_rtp_fault_views(&records, 2, 1).expect_err("reverse_window=1 should fail");
    assert!(matches!(
        udp_err,
        CaptureFaultViewError::InvalidConfig {
            view: "udp_reverse_small_window",
            ..
        }
    ));
}

/// Transport fault views cover TCP, interleaved, UDP, HTTP, and multicast paths.
///
/// 传输层故障视图覆盖 TCP、交错、UDP、HTTP 与组播路径。
#[test]
fn transport_fault_views_cover_tcp_interleaved_udp_http_multicast() {
    let views = build_transport_fault_views(&sample_transport_matrix_records(), 2, 3, 2)
        .expect("transport fault views should build");
    let names: Vec<&str> = views.iter().map(|view| view.name).collect();

    for expected in [
        "transport_tcp_single_buffer",
        "transport_tcp_coalesced_n",
        "transport_tcp_drop_every_nth",
        "transport_interleaved_split_header",
        "transport_interleaved_oversize_length",
        "transport_udp_drop_every_nth",
        "transport_udp_reverse_small_window",
        "transport_http_base64_split_1_3",
        "transport_http_invalid_base64",
        "transport_multicast_drop_every_nth",
    ] {
        assert!(
            names.contains(&expected),
            "missing transport fault view {expected}"
        );
    }
}

/// Build an in-memory `rtspcap` byte vector from a list of synthetic records.
///
/// 根据一组合成记录在内存中构造 `rtspcap` 字节向量。
fn build_rtspcap(records: &[(u8, u8, u16, u32, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RSF1");
    bytes.extend_from_slice(&(records.len() as u32).to_be_bytes());
    for (kind, flags, flow_id, delta_us, payload) in records {
        bytes.push(*kind);
        bytes.push(*flags);
        bytes.extend_from_slice(&flow_id.to_be_bytes());
        bytes.extend_from_slice(&delta_us.to_be_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(payload);
    }
    bytes
}

/// Generate a sample set of capture records for fault-view unit tests.
///
/// 为故障视图单元测试生成一组示例抓包记录。
fn sample_records() -> Vec<CaptureRecord> {
    vec![
        CaptureRecord {
            kind: CaptureRecordKind::RtspTcpC2s,
            flags: 0x01,
            flow_id: 1,
            delta_us: 0,
            payload: b"OPTIONS rtsp://127.0.0.1/live RTSP/1.0\r\nCSeq: 1\r\n\r\n".to_vec(),
        },
        CaptureRecord {
            kind: CaptureRecordKind::RtspTcpC2s,
            flags: 0x01,
            flow_id: 1,
            delta_us: 10,
            payload: b"ANNOUNCE rtsp://127.0.0.1/live RTSP/1.0\r\nCSeq: 2\r\n\r\n".to_vec(),
        },
        CaptureRecord {
            kind: CaptureRecordKind::UdpPublishRtp,
            flags: 0x01,
            flow_id: 2,
            delta_us: 20,
            payload: vec![0x80, 96, 0, 1, 0, 0, 0, 1, 0, 0, 0, 7, 1, 2, 3],
        },
        CaptureRecord {
            kind: CaptureRecordKind::UdpPublishRtp,
            flags: 0x01,
            flow_id: 2,
            delta_us: 30,
            payload: vec![0x80, 96, 0, 2, 0, 0, 0, 2, 0, 0, 0, 7, 4, 5, 6],
        },
        CaptureRecord {
            kind: CaptureRecordKind::UdpPublishRtcp,
            flags: 0x01,
            flow_id: 3,
            delta_us: 40,
            payload: vec![0x80, 200, 0, 6, 0, 0, 0, 7],
        },
    ]
}

/// Generate a sample set that covers TCP, interleaved, UDP, and multicast records.
///
/// 生成覆盖 TCP、交错、UDP 与组播记录的示例集合。
fn sample_transport_matrix_records() -> Vec<CaptureRecord> {
    vec![
        CaptureRecord {
            kind: CaptureRecordKind::RtspTcpC2s,
            flags: 0x01,
            flow_id: 10,
            delta_us: 0,
            payload: b"OPTIONS rtsp://127.0.0.1/live RTSP/1.0\r\nCSeq: 1\r\n\r\n".to_vec(),
        },
        CaptureRecord {
            kind: CaptureRecordKind::RtspTcpS2c,
            flags: 0x01,
            flow_id: 10,
            delta_us: 10,
            payload: b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n".to_vec(),
        },
        CaptureRecord {
            kind: CaptureRecordKind::TcpInterleavedRtp,
            flags: 0x01,
            flow_id: 20,
            delta_us: 20,
            payload: vec![0x80, 96, 0, 1, 0, 0, 0, 1, 0, 0, 0, 7, 1, 2, 3],
        },
        CaptureRecord {
            kind: CaptureRecordKind::TcpInterleavedRtcp,
            flags: 0x01,
            flow_id: 20,
            delta_us: 30,
            payload: vec![0x80, 200, 0, 6, 0, 0, 0, 7],
        },
        CaptureRecord {
            kind: CaptureRecordKind::UdpPublishRtp,
            flags: 0x01,
            flow_id: 30,
            delta_us: 40,
            payload: vec![0x80, 96, 0, 2, 0, 0, 0, 2, 0, 0, 0, 7, 4, 5, 6],
        },
        CaptureRecord {
            kind: CaptureRecordKind::UdpPublishRtcp,
            flags: 0x01,
            flow_id: 30,
            delta_us: 50,
            payload: vec![0x80, 201, 0, 6, 0, 0, 0, 7],
        },
        CaptureRecord {
            kind: CaptureRecordKind::UdpPlayRtp,
            flags: 0x01,
            flow_id: 40,
            delta_us: 60,
            payload: vec![0x80, 96, 0, 3, 0, 0, 0, 3, 0, 0, 0, 7, 7, 8, 9],
        },
        CaptureRecord {
            kind: CaptureRecordKind::UdpPlayRtcp,
            flags: 0x01,
            flow_id: 40,
            delta_us: 70,
            payload: vec![0x80, 202, 0, 6, 0, 0, 0, 7],
        },
    ]
}

/// Create a temporary fixture root for negative-size tests.
///
/// 为大小限制负向测试创建临时 fixture 根目录。
fn create_temp_fixture_root(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create temp fixture root");
    root
}

/// Decode all RTSP requests from a slice of capture records.
///
/// 从抓包记录切片中解码所有 RTSP 请求。
fn decode_requests_from_records(records: &[CaptureRecord]) -> Result<Vec<RtspRequest>, String> {
    let mut core = RtspCore::new();
    let mut requests = Vec::new();
    for record in records
        .iter()
        .filter(|record| record.kind == CaptureRecordKind::RtspTcpC2s)
    {
        let outputs = core
            .handle_input(CoreInput::Bytes(Bytes::copy_from_slice(&record.payload)))
            .map_err(|err| err.to_string())?;
        for output in outputs {
            if let CoreOutput::Event(RtspEvent::Request(request)) = output {
                requests.push(request);
            }
        }
    }
    Ok(requests)
}

/// Count the number of RTP and RTCP packets in the records.
///
/// 统计记录中的 RTP 与 RTCP 包数量。
fn count_rtp_rtcp_packets(records: &[CaptureRecord]) -> (usize, usize) {
    let mut rtp_count = 0usize;
    let mut rtcp_count = 0usize;

    for record in records {
        match record.kind {
            CaptureRecordKind::UdpPublishRtp
            | CaptureRecordKind::UdpPlayRtp
            | CaptureRecordKind::TcpInterleavedRtp => {
                rtp_count += 1;
            }
            CaptureRecordKind::UdpPublishRtcp
            | CaptureRecordKind::UdpPlayRtcp
            | CaptureRecordKind::TcpInterleavedRtcp => {
                rtcp_count += 1;
            }
            CaptureRecordKind::RtspTcpC2s | CaptureRecordKind::RtspTcpS2c => {}
        }
    }

    (rtp_count, rtcp_count)
}

/// Collect the unique track control suffixes from SETUP requests.
///
/// 从 SETUP 请求收集唯一的 track control 后缀。
fn unique_setup_track_controls(requests: &[RtspRequest]) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for request in requests {
        if request.method != RtspMethod::Setup {
            continue;
        }
        if let Some(control) = request.uri.rsplit('/').next() {
            out.insert(control.to_owned());
        }
    }
    out
}

/// Check whether a media signature string indicates an audio-only fixture.
///
/// 检查媒体签名字符串是否表示仅音频 fixture。
fn is_audio_only_media_sig(media_sig: &str) -> bool {
    media_sig.starts_with("v=none@")
}
