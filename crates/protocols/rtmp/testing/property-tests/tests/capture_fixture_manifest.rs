//! Unit tests for the capture fixture manifest parser and `rtmpflow` decoder.
//!
//! 抓包 fixture manifest 解析器与 `rtmpflow` 解码器的单元测试。

#[allow(dead_code)]
#[path = "support/capture_fixture.rs"]
mod capture_fixture;

use std::path::Path;

use capture_fixture::{
    decode_rtmpflow, parse_manifest, validate_manifest, CaptureFixtureError, CaptureRole,
    MANIFEST_HEADER,
};

const MANIFEST: &str = include_str!("testdata/rtmp-capture/manifest.tsv");

/// Verify the committed manifest parses, validates, and follows expected invariants.
///
/// 校验已提交的 manifest 能解析、验证，并满足预期不变量。
#[test]
fn committed_manifest_header_is_valid() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/rtmp-capture");
    let rows = validate_manifest(&root, MANIFEST).expect("committed manifest should be valid");
    assert_eq!(
        rows.len(),
        8,
        "1.3 commits four standard and four probe fixtures"
    );

    let standard_rows = rows
        .iter()
        .filter(|row| row.role == CaptureRole::ServerPublishC2s)
        .count();
    let probe_rows = rows
        .iter()
        .filter(|row| row.role == CaptureRole::RobustnessProbe)
        .count();
    assert_eq!(standard_rows, 4);
    assert_eq!(probe_rows, 4);

    for row in rows {
        match row.role {
            CaptureRole::ServerPublishC2s => {
                assert!(row.expect_connected, "standard case should expect connect");
                assert!(row.expect_publish, "standard case should expect publish");
                assert!(!row.expect_play, "publish fixtures should not expect play");
                assert!(
                    row.expect_media_min >= 1,
                    "standard case should expect media"
                );
                assert!(
                    row.fixture.starts_with("standard"),
                    "standard case should live under standard/"
                );
            }
            CaptureRole::RobustnessProbe => {
                assert!(
                    !row.expect_connected,
                    "probe should not require connect success"
                );
                assert!(
                    !row.expect_publish,
                    "probe should not require publish success"
                );
                assert!(!row.expect_play, "probe should not require play success");
                assert_eq!(row.expect_media_min, 0, "probe should not require media");
                assert!(
                    row.notes.contains("probe"),
                    "probe notes should make compatibility scope explicit"
                );
                assert!(
                    row.fixture.starts_with("probes"),
                    "probe case should live under probes/"
                );
            }
            other => panic!("unexpected committed fixture role: {other:?}"),
        }
    }
}

/// If `RTMP_CAPTURE_FIXTURE_DIR` is set, validate the generated manifest too.
///
/// 若设置了 `RTMP_CAPTURE_FIXTURE_DIR`，也校验生成的 manifest。
#[test]
fn generated_manifest_from_env_is_valid() {
    let Ok(root) = std::env::var("RTMP_CAPTURE_FIXTURE_DIR") else {
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

/// Verify that a valid `rtmpflow` with two records decodes correctly.
///
/// 校验包含两条记录的有效 `rtmpflow` 能正确解码。
#[test]
fn decode_rtmpflow_accepts_valid_records() {
    let bytes = build_rtmpflow(&[b"first".as_slice(), b"second".as_slice()]);
    let records = decode_rtmpflow(&bytes).expect("valid rtmpflow should decode");

    assert_eq!(records, vec![b"first".as_slice(), b"second".as_slice()]);
}

/// Verify that an incorrect magic header is rejected.
///
/// 校验错误的魔数头会被拒绝。
#[test]
fn decode_rtmpflow_rejects_bad_magic() {
    let mut bytes = build_rtmpflow(&[b"payload".as_slice()]);
    bytes[0..4].copy_from_slice(b"NOPE");

    assert!(matches!(
        decode_rtmpflow(&bytes),
        Err(CaptureFixtureError::BadMagic)
    ));
}

/// Verify that a truncated payload is rejected.
///
/// 校验截断的 payload 会被拒绝。
#[test]
fn decode_rtmpflow_rejects_truncated_payload() {
    let mut bytes = build_rtmpflow(&[b"payload".as_slice()]);
    bytes.pop();

    assert!(matches!(
        decode_rtmpflow(&bytes),
        Err(CaptureFixtureError::Truncated { .. })
    ));
}

/// Verify that a zero-length record is rejected.
///
/// 校验零长度记录会被拒绝。
#[test]
fn decode_rtmpflow_rejects_zero_length_record() {
    let bytes = build_rtmpflow(&[b"".as_slice()]);

    assert!(matches!(
        decode_rtmpflow(&bytes),
        Err(CaptureFixtureError::ZeroLengthRecord { index: 0 })
    ));
}

/// Verify that trailing bytes after the last record are rejected.
///
/// 校验最后一条记录后的多余字节会被拒绝。
#[test]
fn decode_rtmpflow_rejects_trailing_bytes() {
    let mut bytes = build_rtmpflow(&[b"payload".as_slice()]);
    bytes.push(0);

    assert!(matches!(
        decode_rtmpflow(&bytes),
        Err(CaptureFixtureError::TrailingBytes { bytes: 1 })
    ));
}

/// Verify that a mismatched manifest header is rejected.
///
/// 校验不匹配的 manifest 表头会被拒绝。
#[test]
fn manifest_rejects_bad_header() {
    let err = parse_manifest("bad\theader\n").expect_err("bad header should fail");

    assert!(matches!(
        err,
        CaptureFixtureError::InvalidManifestHeader { .. }
    ));
}

/// Verify that a row with the wrong number of fields is rejected.
///
/// 校验字段数量不正确的行会被拒绝。
#[test]
fn manifest_rejects_bad_field_count() {
    let input = format!("{MANIFEST_HEADER}\ncase\ttoo-short\n");
    let err = parse_manifest(&input).expect_err("short row should fail");

    assert!(matches!(
        err,
        CaptureFixtureError::InvalidManifestFieldCount {
            line: 2,
            expected: 11,
            actual: 2
        }
    ));
}

/// Verify that an unknown role value is rejected.
///
/// 校验未知的角色值会被拒绝。
#[test]
fn manifest_rejects_invalid_role() {
    let input = format!(
        "{MANIFEST_HEADER}\ncase\tcapture.pcap\tstream\tv=h264@1x1;a=aac@ch2\tunknown\tstandard/case.rtmpflow\t1\t1\t0\t1\tnote\n"
    );
    let err = parse_manifest(&input).expect_err("invalid role should fail");

    assert!(matches!(
        err,
        CaptureFixtureError::InvalidRole { line: 2, .. }
    ));
}

/// Verify that fixture paths escaping the root directory are rejected.
///
/// 校验试图逃逸根目录的 fixture 路径会被拒绝。
#[test]
fn manifest_rejects_unsafe_fixture_path() {
    let input = format!(
        "{MANIFEST_HEADER}\ncase\tcapture.pcap\tstream\tv=h264@1x1;a=aac@ch2\tserver_publish_c2s\t../case.rtmpflow\t1\t1\t0\t1\tnote\n"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/rtmp-capture");
    let err = validate_manifest(&root, &input).expect_err("unsafe path should fail");

    assert!(matches!(err, CaptureFixtureError::UnsafeFixturePath { .. }));
}

/// Build a minimal `rtmpflow` file from record payloads for testing.
///
/// 从记录 payload 构建最小 `rtmpflow` 文件，用于测试。
fn build_rtmpflow(records: &[&[u8]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"CRF1");
    bytes.extend_from_slice(&(records.len() as u32).to_be_bytes());
    for record in records {
        bytes.extend_from_slice(&(record.len() as u32).to_be_bytes());
        bytes.extend_from_slice(record);
    }
    bytes
}
