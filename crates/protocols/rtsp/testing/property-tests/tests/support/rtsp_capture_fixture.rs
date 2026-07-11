//! Capture fixture loader and fault-view generator for RTSP property tests.
//!
//! The `rtspcap` format is a simple binary container of timestamped records. This
//! module parses the manifest, loads fixtures, decodes the binary records, and
//! exposes `NamedPayloadView` generators that simulate TCP/UDP/interleaved/
//! HTTP/multicast transport faults.
//!
//! RTSP 属性测试的抓包 fixture 加载器与故障视图生成器。
//!
//! `rtspcap` 格式是一种带时间戳记录的二进制简单容器。本模块解析清单、加载
//! fixture、解码二进制记录，并暴露 `NamedPayloadView` 生成器以模拟 TCP/UDP/交错/
//! HTTP/组播传输故障。

use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// TSV header that must match the first line of `manifest.tsv`.
///
/// `manifest.tsv` 第一行必须匹配的 TSV 表头。
pub const MANIFEST_HEADER: &str = "case\tsource_pcap\tstream_name\tmedia_sig\tpush_transport\tpull_transport\trole\tfixture\texpect_methods\texpect_rtp_min\texpect_rtcp_min\texpect_tracks_min\tnotes";

/// Maximum fixture file size in bytes.
///
/// fixture 文件的最大字节数。
pub const MAX_FIXTURE_BYTES: u64 = 524_288;

/// Magic four bytes for the `rtspcap` binary format.
///
/// `rtspcap` 二进制格式魔数。
const RTSPCAP_MAGIC: &[u8; 4] = b"RSF1";

/// Number of tab-separated fields in each manifest row.
///
/// 清单每行中制表符分隔的字段数。
const MANIFEST_FIELD_COUNT: usize = 13;

/// Accepted record flag bits for `rtspcap` records.
///
/// `rtspcap` 记录可接受的标志位。
const KNOWN_RECORD_FLAGS: u8 = 0x01 | 0x02 | 0x04;

/// Transport used for the RTSP stream path.
///
/// RTSP 流路径使用的传输方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTransport {
    Tcp,
    Udp,
    HttpTunnel,
    Multicast,
    Mixed,
    None,
}

impl CaptureTransport {
    /// Parse a transport string from the manifest.
    ///
    /// 从清单中解析传输字符串。
    fn parse(line: usize, field: &'static str, value: &str) -> Result<Self, CaptureFixtureError> {
        match value {
            "tcp" => Ok(Self::Tcp),
            "udp" => Ok(Self::Udp),
            "http-tunnel" => Ok(Self::HttpTunnel),
            "multicast" => Ok(Self::Multicast),
            "mixed" => Ok(Self::Mixed),
            "none" => Ok(Self::None),
            _ => Err(CaptureFixtureError::InvalidTransport {
                line,
                field,
                value: value.to_owned(),
            }),
        }
    }
}

/// Role of the capture fixture in the test matrix.
///
/// 抓包 fixture 在测试矩阵中的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRole {
    StandardPublishTcp,
    StandardPublishUdp,
    StandardPublishHttpTunnel,
    StandardPlayTcp,
    StandardPlayUdp,
    StandardPlayMulticast,
    StandardPullJob,
    StandardPushJob,
    StandardRelayJob,
    CompatProbe,
    TransportFaultSeed,
}

impl CaptureRole {
    /// Parse a role string from the manifest.
    ///
    /// 从清单中解析角色字符串。
    fn parse(line: usize, value: &str) -> Result<Self, CaptureFixtureError> {
        match value {
            "standard_publish_tcp" => Ok(Self::StandardPublishTcp),
            "standard_publish_udp" => Ok(Self::StandardPublishUdp),
            "standard_publish_http_tunnel" => Ok(Self::StandardPublishHttpTunnel),
            "standard_play_tcp" => Ok(Self::StandardPlayTcp),
            "standard_play_udp" => Ok(Self::StandardPlayUdp),
            "standard_play_multicast" => Ok(Self::StandardPlayMulticast),
            "standard_pull_job" => Ok(Self::StandardPullJob),
            "standard_push_job" => Ok(Self::StandardPushJob),
            "standard_relay_job" => Ok(Self::StandardRelayJob),
            "compat_probe" => Ok(Self::CompatProbe),
            "transport_fault_seed" => Ok(Self::TransportFaultSeed),
            _ => Err(CaptureFixtureError::InvalidRole {
                line,
                value: value.to_owned(),
            }),
        }
    }
}

/// Kind of a record in the `rtspcap` binary container.
///
/// `rtspcap` 二进制容器中的记录类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRecordKind {
    RtspTcpC2s,
    RtspTcpS2c,
    UdpPublishRtp,
    UdpPublishRtcp,
    UdpPlayRtp,
    UdpPlayRtcp,
    TcpInterleavedRtp,
    TcpInterleavedRtcp,
}

impl CaptureRecordKind {
    /// Parse a one-byte record kind.
    ///
    /// 解析一字节记录类型。
    fn parse(raw: u8) -> Result<Self, CaptureFixtureError> {
        match raw {
            1 => Ok(Self::RtspTcpC2s),
            2 => Ok(Self::RtspTcpS2c),
            3 => Ok(Self::UdpPublishRtp),
            4 => Ok(Self::UdpPublishRtcp),
            5 => Ok(Self::UdpPlayRtp),
            6 => Ok(Self::UdpPlayRtcp),
            7 => Ok(Self::TcpInterleavedRtp),
            8 => Ok(Self::TcpInterleavedRtcp),
            _ => Err(CaptureFixtureError::InvalidRecordKind { raw }),
        }
    }
}

/// One row of the capture fixture manifest.
///
/// 抓包 fixture 清单的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureManifestRow {
    pub line: usize,
    pub case: String,
    pub source_pcap: String,
    pub stream_name: String,
    pub media_sig: String,
    pub push_transport: CaptureTransport,
    pub pull_transport: CaptureTransport,
    pub role: CaptureRole,
    pub fixture: PathBuf,
    pub expect_methods: Vec<String>,
    pub expect_rtp_min: usize,
    pub expect_rtcp_min: usize,
    pub expect_tracks_min: usize,
    pub notes: String,
}

/// One decoded record from the `rtspcap` binary container.
///
/// 从 `rtspcap` 二进制容器解码出的一条记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRecord {
    pub kind: CaptureRecordKind,
    pub flags: u8,
    pub flow_id: u16,
    pub delta_us: u32,
    pub payload: Vec<u8>,
}

/// A loaded fixture: manifest row plus decoded records.
///
/// 加载后的 fixture：清单行与解码记录。
#[derive(Debug, Clone)]
pub struct CaptureFixture {
    pub row: CaptureManifestRow,
    pub records: Vec<CaptureRecord>,
}

/// A named `Vec<Vec<u8>>` view of a payload stream used for fault injection.
///
/// 用于故障注入的 payload 流具名视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedPayloadView {
    pub name: &'static str,
    pub payloads: Vec<Vec<u8>>,
}

/// Error produced when building fault views.
///
/// 构造故障视图时产生的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFaultViewError {
    InvalidConfig { view: &'static str, detail: String },
    EmptyInput { view: &'static str },
}

/// Error produced when loading or validating capture fixtures.
///
/// 加载或验证抓包 fixture 时产生的错误。
#[derive(Debug)]
pub enum CaptureFixtureError {
    MissingManifestHeader,
    InvalidManifestHeader {
        expected: &'static str,
        actual: String,
    },
    InvalidManifestFieldCount {
        line: usize,
        expected: usize,
        actual: usize,
    },
    EmptyManifestField {
        line: usize,
        field: &'static str,
    },
    InvalidRole {
        line: usize,
        value: String,
    },
    InvalidTransport {
        line: usize,
        field: &'static str,
        value: String,
    },
    InvalidNumber {
        line: usize,
        field: &'static str,
        value: String,
    },
    InvalidMethods {
        line: usize,
        value: String,
    },
    UnsafeFixturePath {
        line: usize,
        path: PathBuf,
    },
    MissingFixture {
        line: usize,
        path: PathBuf,
    },
    FixtureRead {
        path: PathBuf,
        source: io::Error,
    },
    FixtureTooLarge {
        line: usize,
        path: PathBuf,
        bytes: u64,
        max: u64,
    },
    BadMagic,
    Truncated {
        context: String,
    },
    ZeroLengthRecord {
        index: usize,
    },
    InvalidRecordKind {
        raw: u8,
    },
    InvalidRecordFlags {
        raw: u8,
    },
    RecordCountMismatch {
        expected: u32,
        actual: usize,
    },
    TrailingBytes {
        bytes: usize,
    },
}

impl fmt::Display for CaptureFixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingManifestHeader => write!(f, "missing manifest header"),
            Self::InvalidManifestHeader { expected, actual } => {
                write!(
                    f,
                    "invalid manifest header: expected {expected:?}, got {actual:?}"
                )
            }
            Self::InvalidManifestFieldCount {
                line,
                expected,
                actual,
            } => write!(
                f,
                "invalid manifest field count at line {line}: expected {expected}, got {actual}"
            ),
            Self::EmptyManifestField { line, field } => {
                write!(f, "empty manifest field {field:?} at line {line}")
            }
            Self::InvalidRole { line, value } => {
                write!(f, "invalid role {value:?} at line {line}")
            }
            Self::InvalidTransport { line, field, value } => {
                write!(f, "invalid transport {field:?}={value:?} at line {line}")
            }
            Self::InvalidNumber { line, field, value } => {
                write!(f, "invalid number {field:?}={value:?} at line {line}")
            }
            Self::InvalidMethods { line, value } => {
                write!(f, "invalid expect_methods {value:?} at line {line}")
            }
            Self::UnsafeFixturePath { line, path } => {
                write!(f, "unsafe fixture path {path:?} at line {line}")
            }
            Self::MissingFixture { line, path } => {
                write!(f, "missing fixture {path:?} at line {line}")
            }
            Self::FixtureRead { path, source } => {
                write!(f, "failed to read fixture {path:?}: {source}")
            }
            Self::FixtureTooLarge {
                line,
                path,
                bytes,
                max,
            } => write!(
                f,
                "fixture {path:?} at line {line} is too large: {bytes} bytes > {max}"
            ),
            Self::BadMagic => write!(f, "invalid rtspcap magic"),
            Self::Truncated { context } => write!(f, "truncated rtspcap while reading {context}"),
            Self::ZeroLengthRecord { index } => {
                write!(f, "rtspcap record {index} has zero length")
            }
            Self::InvalidRecordKind { raw } => write!(f, "invalid rtspcap record kind {raw}"),
            Self::InvalidRecordFlags { raw } => {
                write!(f, "invalid rtspcap record flags 0x{raw:02x}")
            }
            Self::RecordCountMismatch { expected, actual } => write!(
                f,
                "record count mismatch: expected {expected}, got {actual}"
            ),
            Self::TrailingBytes { bytes } => write!(f, "rtspcap has {bytes} trailing bytes"),
        }
    }
}

impl std::error::Error for CaptureFixtureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FixtureRead { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl fmt::Display for CaptureFaultViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig { view, detail } => {
                write!(f, "invalid fault view config for {view}: {detail}")
            }
            Self::EmptyInput { view } => {
                write!(f, "cannot build fault view {view} from empty input")
            }
        }
    }
}

impl std::error::Error for CaptureFaultViewError {}

/// Parse the manifest TSV into rows.
///
/// 将清单 TSV 解析为行。
pub fn parse_manifest(input: &str) -> Result<Vec<CaptureManifestRow>, CaptureFixtureError> {
    let mut lines = input.lines();
    let header = lines
        .next()
        .ok_or(CaptureFixtureError::MissingManifestHeader)?;
    if header != MANIFEST_HEADER {
        return Err(CaptureFixtureError::InvalidManifestHeader {
            expected: MANIFEST_HEADER,
            actual: header.to_owned(),
        });
    }

    let mut rows = Vec::new();
    for (idx, line) in lines.enumerate() {
        let line_number = idx + 2;
        if line.trim().is_empty() {
            continue;
        }

        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != MANIFEST_FIELD_COUNT {
            return Err(CaptureFixtureError::InvalidManifestFieldCount {
                line: line_number,
                expected: MANIFEST_FIELD_COUNT,
                actual: fields.len(),
            });
        }

        for (field, value) in [
            ("case", fields[0]),
            ("source_pcap", fields[1]),
            ("stream_name", fields[2]),
            ("media_sig", fields[3]),
            ("push_transport", fields[4]),
            ("pull_transport", fields[5]),
            ("role", fields[6]),
            ("fixture", fields[7]),
            ("expect_methods", fields[8]),
            ("expect_rtp_min", fields[9]),
            ("expect_rtcp_min", fields[10]),
            ("expect_tracks_min", fields[11]),
        ] {
            if value.trim().is_empty() {
                return Err(CaptureFixtureError::EmptyManifestField {
                    line: line_number,
                    field,
                });
            }
        }

        rows.push(CaptureManifestRow {
            line: line_number,
            case: fields[0].to_owned(),
            source_pcap: fields[1].to_owned(),
            stream_name: fields[2].to_owned(),
            media_sig: fields[3].to_owned(),
            push_transport: CaptureTransport::parse(line_number, "push_transport", fields[4])?,
            pull_transport: CaptureTransport::parse(line_number, "pull_transport", fields[5])?,
            role: CaptureRole::parse(line_number, fields[6])?,
            fixture: PathBuf::from(fields[7]),
            expect_methods: parse_expect_methods(line_number, fields[8])?,
            expect_rtp_min: parse_number(line_number, "expect_rtp_min", fields[9])?,
            expect_rtcp_min: parse_number(line_number, "expect_rtcp_min", fields[10])?,
            expect_tracks_min: parse_number(line_number, "expect_tracks_min", fields[11])?,
            notes: fields[12].to_owned(),
        });
    }

    Ok(rows)
}

/// Parse and validate the manifest, including fixture existence, size, and decode.
///
/// 解析并验证清单，包括 fixture 存在性、大小与可解码性。
pub fn validate_manifest(
    root: &Path,
    input: &str,
) -> Result<Vec<CaptureManifestRow>, CaptureFixtureError> {
    let rows = parse_manifest(input)?;
    for row in &rows {
        validate_fixture_path(row.line, &row.fixture)?;
        let full_path = root.join(&row.fixture);
        let metadata = fs::metadata(&full_path).map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                CaptureFixtureError::MissingFixture {
                    line: row.line,
                    path: row.fixture.clone(),
                }
            } else {
                CaptureFixtureError::FixtureRead {
                    path: full_path.clone(),
                    source: err,
                }
            }
        })?;

        if metadata.len() > MAX_FIXTURE_BYTES {
            return Err(CaptureFixtureError::FixtureTooLarge {
                line: row.line,
                path: row.fixture.clone(),
                bytes: metadata.len(),
                max: MAX_FIXTURE_BYTES,
            });
        }

        let bytes = fs::read(&full_path).map_err(|source| CaptureFixtureError::FixtureRead {
            path: full_path,
            source,
        })?;
        decode_rtspcap(&bytes)?;
    }

    Ok(rows)
}

/// Load all fixtures validated by the manifest.
///
/// 加载清单验证通过的所有 fixture。
pub fn load_capture_fixtures(
    root: &Path,
    input: &str,
) -> Result<Vec<CaptureFixture>, CaptureFixtureError> {
    let rows = validate_manifest(root, input)?;
    let mut fixtures = Vec::with_capacity(rows.len());
    for row in rows {
        let full_path = root.join(&row.fixture);
        let bytes = fs::read(&full_path).map_err(|source| CaptureFixtureError::FixtureRead {
            path: full_path,
            source,
        })?;
        let records = decode_rtspcap(&bytes)?;
        fixtures.push(CaptureFixture { row, records });
    }
    Ok(fixtures)
}

/// Decode a `rtspcap` byte buffer into a list of records.
///
/// 将 `rtspcap` 字节缓冲区解码为记录列表。
pub fn decode_rtspcap(bytes: &[u8]) -> Result<Vec<CaptureRecord>, CaptureFixtureError> {
    if bytes.len() < 8 {
        return Err(CaptureFixtureError::Truncated {
            context: "header".to_owned(),
        });
    }
    if &bytes[..4] != RTSPCAP_MAGIC {
        return Err(CaptureFixtureError::BadMagic);
    }

    let mut cursor = 4;
    let expected = read_u32(bytes, &mut cursor, "record_count")?;
    let mut records = Vec::with_capacity(expected as usize);

    for index in 0..(expected as usize) {
        let kind_raw = read_u8(bytes, &mut cursor, "kind")?;
        let kind = CaptureRecordKind::parse(kind_raw)?;
        let flags = read_u8(bytes, &mut cursor, "flags")?;
        validate_record_flags(flags)?;
        let flow_id = read_u16(bytes, &mut cursor, "flow_id")?;
        let delta_us = read_u32(bytes, &mut cursor, "delta_us")?;
        let payload_len = read_u32(bytes, &mut cursor, "payload_len")? as usize;
        if payload_len == 0 {
            return Err(CaptureFixtureError::ZeroLengthRecord { index });
        }
        let payload = read_bytes(bytes, &mut cursor, payload_len, "payload")?.to_vec();

        records.push(CaptureRecord {
            kind,
            flags,
            flow_id,
            delta_us,
            payload,
        });
    }

    if records.len() != expected as usize {
        return Err(CaptureFixtureError::RecordCountMismatch {
            expected,
            actual: records.len(),
        });
    }

    if cursor != bytes.len() {
        return Err(CaptureFixtureError::TrailingBytes {
            bytes: bytes.len() - cursor,
        });
    }

    Ok(records)
}

/// Build TCP-oriented fault views for a set of capture records.
///
/// 为一组抓包记录构造面向 TCP 的故障视图。
pub fn build_tcp_fault_views(
    records: &[CaptureRecord],
    coalesced_n: usize,
    drop_every_nth: usize,
) -> Result<Vec<NamedPayloadView>, CaptureFaultViewError> {
    if coalesced_n < 2 {
        return Err(CaptureFaultViewError::InvalidConfig {
            view: "tcp_coalesced_n",
            detail: format!("coalesced_n must be >= 2, got {coalesced_n}"),
        });
    }
    if drop_every_nth < 2 {
        return Err(CaptureFaultViewError::InvalidConfig {
            view: "tcp_drop_every_nth",
            detail: format!("drop_every_nth must be >= 2, got {drop_every_nth}"),
        });
    }

    let payloads = tcp_payload_records(records);
    if payloads.is_empty() {
        return Err(CaptureFaultViewError::EmptyInput {
            view: "tcp_fault_views",
        });
    }

    Ok(vec![
        NamedPayloadView {
            name: "tcp_single_buffer",
            payloads: vec![payloads.concat()],
        },
        NamedPayloadView {
            name: "tcp_original_records",
            payloads: payloads.clone(),
        },
        NamedPayloadView {
            name: "tcp_one_byte_chunks",
            payloads: split_one_byte_chunks(&payloads),
        },
        NamedPayloadView {
            name: "tcp_coalesced_n",
            payloads: coalesced_chunks(&payloads, coalesced_n),
        },
        NamedPayloadView {
            name: "tcp_prefix_truncated_half",
            payloads: prefix_by_bytes(&payloads, total_payload_len(&payloads) / 2),
        },
        NamedPayloadView {
            name: "tcp_duplicate_record",
            payloads: duplicate_first_payload(&payloads),
        },
        NamedPayloadView {
            name: "tcp_swap_adjacent",
            payloads: swap_adjacent(&payloads),
        },
        NamedPayloadView {
            name: "tcp_drop_every_nth",
            payloads: drop_every_nth_payload(&payloads, drop_every_nth),
        },
    ])
}

/// Build UDP/RTP-oriented fault views for a set of capture records.
///
/// 为一组抓包记录构造面向 UDP/RTP 的故障视图。
pub fn build_udp_rtp_fault_views(
    records: &[CaptureRecord],
    drop_every_nth: usize,
    reverse_window: usize,
) -> Result<Vec<NamedPayloadView>, CaptureFaultViewError> {
    if drop_every_nth < 2 {
        return Err(CaptureFaultViewError::InvalidConfig {
            view: "udp_drop_every_nth",
            detail: format!("drop_every_nth must be >= 2, got {drop_every_nth}"),
        });
    }
    if reverse_window < 2 {
        return Err(CaptureFaultViewError::InvalidConfig {
            view: "udp_reverse_small_window",
            detail: format!("reverse_window must be >= 2, got {reverse_window}"),
        });
    }

    let payloads = udp_payload_records(records);
    if payloads.is_empty() {
        return Err(CaptureFaultViewError::EmptyInput {
            view: "udp_fault_views",
        });
    }

    Ok(vec![
        NamedPayloadView {
            name: "udp_drop_datagram",
            payloads: drop_every_nth_payload(&payloads, drop_every_nth),
        },
        NamedPayloadView {
            name: "udp_duplicate_datagram",
            payloads: duplicate_first_payload(&payloads),
        },
        NamedPayloadView {
            name: "udp_swap_adjacent_datagrams",
            payloads: swap_adjacent(&payloads),
        },
        NamedPayloadView {
            name: "udp_reverse_small_window",
            payloads: reverse_small_windows(&payloads, reverse_window),
        },
        NamedPayloadView {
            name: "udp_truncate_payload",
            payloads: truncate_payloads_half(&payloads),
        },
        NamedPayloadView {
            name: "rtp_sequence_reorder",
            payloads: reorder_rtp_sequence(&payloads),
        },
    ])
}

/// Build transport-layer fault views combining TCP, UDP, interleaved, HTTP, and multicast paths.
///
/// 构造结合 TCP、UDP、交错、HTTP 与组播路径的传输层故障视图。
pub fn build_transport_fault_views(
    records: &[CaptureRecord],
    coalesced_n: usize,
    drop_every_nth: usize,
    reverse_window: usize,
) -> Result<Vec<NamedPayloadView>, CaptureFaultViewError> {
    if coalesced_n < 2 {
        return Err(CaptureFaultViewError::InvalidConfig {
            view: "transport_tcp_coalesced_n",
            detail: format!("coalesced_n must be >= 2, got {coalesced_n}"),
        });
    }
    if drop_every_nth < 2 {
        return Err(CaptureFaultViewError::InvalidConfig {
            view: "transport_drop_every_nth",
            detail: format!("drop_every_nth must be >= 2, got {drop_every_nth}"),
        });
    }
    if reverse_window < 2 {
        return Err(CaptureFaultViewError::InvalidConfig {
            view: "transport_udp_reverse_small_window",
            detail: format!("reverse_window must be >= 2, got {reverse_window}"),
        });
    }

    let tcp_payloads = tcp_payload_records(records);
    let udp_payloads = udp_payload_records(records);
    let interleaved_frames = interleaved_payload_records(records);
    let multicast_payloads = multicast_payload_records(records);

    if tcp_payloads.is_empty()
        && udp_payloads.is_empty()
        && interleaved_frames.is_empty()
        && multicast_payloads.is_empty()
    {
        return Err(CaptureFaultViewError::EmptyInput {
            view: "transport_fault_views",
        });
    }

    let mut views = Vec::new();

    if !tcp_payloads.is_empty() {
        views.push(NamedPayloadView {
            name: "transport_tcp_single_buffer",
            payloads: vec![tcp_payloads.concat()],
        });
        views.push(NamedPayloadView {
            name: "transport_tcp_coalesced_n",
            payloads: coalesced_chunks(&tcp_payloads, coalesced_n),
        });
        views.push(NamedPayloadView {
            name: "transport_tcp_drop_every_nth",
            payloads: drop_every_nth_payload(&tcp_payloads, drop_every_nth),
        });
    }

    if !interleaved_frames.is_empty() {
        let interleaved_stream = encode_interleaved_frames(&interleaved_frames);
        views.push(NamedPayloadView {
            name: "transport_interleaved_split_header",
            payloads: split_interleaved_header(&interleaved_stream),
        });
        views.push(NamedPayloadView {
            name: "transport_interleaved_oversize_length",
            payloads: vec![build_interleaved_oversize_frame(&interleaved_frames[0])],
        });
    }

    if !udp_payloads.is_empty() {
        views.push(NamedPayloadView {
            name: "transport_udp_drop_every_nth",
            payloads: drop_every_nth_payload(&udp_payloads, drop_every_nth),
        });
        views.push(NamedPayloadView {
            name: "transport_udp_reverse_small_window",
            payloads: reverse_small_windows(&udp_payloads, reverse_window),
        });
    }

    if !tcp_payloads.is_empty() {
        let base64_stream = base64_encode(&tcp_payloads.concat());
        views.push(NamedPayloadView {
            name: "transport_http_base64_split_1_3",
            payloads: split_http_base64_1_3(&base64_stream),
        });
        views.push(NamedPayloadView {
            name: "transport_http_invalid_base64",
            payloads: vec![build_invalid_base64_payload(&base64_stream)],
        });
    }

    if !multicast_payloads.is_empty() {
        views.push(NamedPayloadView {
            name: "transport_multicast_drop_every_nth",
            payloads: drop_every_nth_payload(&multicast_payloads, drop_every_nth),
        });
    }

    Ok(views)
}

/// Extract TCP record payloads from capture records.
///
/// 从抓包记录中提取 TCP 记录 payload。
pub fn tcp_payload_records(records: &[CaptureRecord]) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                CaptureRecordKind::RtspTcpC2s | CaptureRecordKind::RtspTcpS2c
            )
        })
        .map(|record| record.payload.clone())
        .collect()
}

/// Extract UDP/RTP/RTCP record payloads from capture records.
///
/// 从抓包记录中提取 UDP/RTP/RTCP 记录 payload。
pub fn udp_payload_records(records: &[CaptureRecord]) -> Vec<Vec<u8>> {
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
        .collect()
}

/// Extract RTSP-over-TCP interleaved (RTP/RTCP) channel frames with their channel id.
///
/// 提取 RTSP-over-TCP 交错（RTP/RTCP）通道帧及其通道 id。
fn interleaved_payload_records(records: &[CaptureRecord]) -> Vec<(u8, Vec<u8>)> {
    records
        .iter()
        .filter_map(|record| match record.kind {
            CaptureRecordKind::TcpInterleavedRtp => Some((0u8, record.payload.clone())),
            CaptureRecordKind::TcpInterleavedRtcp => Some((1u8, record.payload.clone())),
            _ => None,
        })
        .collect()
}

/// Extract multicast play payloads (UDP play RTP/RTCP) from capture records.
///
/// 从抓包记录中提取组播播放 payload（UDP play RTP/RTCP）。
fn multicast_payload_records(records: &[CaptureRecord]) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                CaptureRecordKind::UdpPlayRtp | CaptureRecordKind::UdpPlayRtcp
            )
        })
        .map(|record| record.payload.clone())
        .collect()
}

/// Split each payload into one-byte chunks.
///
/// 将每个 payload 切分为单字节块。
fn split_one_byte_chunks(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    payloads
        .iter()
        .flat_map(|payload| payload.iter().map(|byte| vec![*byte]))
        .collect()
}

/// Merge payloads into chunks of `n` contiguous payloads.
///
/// 将 payload 合并为每 `n` 个连续 payload 一组的 chunk。
fn coalesced_chunks(payloads: &[Vec<u8>], n: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for chunk in payloads.chunks(n) {
        let mut merged = Vec::new();
        for payload in chunk {
            merged.extend_from_slice(payload);
        }
        out.push(merged);
    }
    out
}

/// Return the total byte count of all payloads.
///
/// 返回所有 payload 的总字节数。
fn total_payload_len(payloads: &[Vec<u8>]) -> usize {
    payloads.iter().map(Vec::len).sum()
}

/// Keep the leading `max_bytes` of the payload stream as one or more chunks.
///
/// 仅保留 payload 流的前 `max_bytes` 字节作为一个或多个 chunk。
fn prefix_by_bytes(payloads: &[Vec<u8>], max_bytes: usize) -> Vec<Vec<u8>> {
    if max_bytes == 0 {
        return payloads
            .first()
            .map(|payload| vec![payload[0..payload.len().min(1)].to_vec()])
            .unwrap_or_default();
    }

    let mut out = Vec::new();
    let mut remaining = max_bytes;
    for payload in payloads {
        if remaining == 0 {
            break;
        }
        let take = remaining.min(payload.len());
        if take > 0 {
            out.push(payload[..take].to_vec());
            remaining -= take;
        }
    }

    if out.is_empty() && !payloads.is_empty() {
        out.push(payloads[0][..payloads[0].len().min(1)].to_vec());
    }
    out
}

/// Duplicate the first payload immediately after itself.
///
/// 将第一个 payload 紧接自身复制一次。
fn duplicate_first_payload(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    if payloads.is_empty() {
        return Vec::new();
    }
    let mut out = payloads.to_vec();
    out.insert(1, payloads[0].clone());
    out
}

/// Swap the first two payloads if they exist.
///
/// 若存在则交换前两个 payload。
fn swap_adjacent(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = payloads.to_vec();
    if out.len() >= 2 {
        out.swap(0, 1);
    }
    out
}

/// Drop every `n`th payload, ensuring the result is non-empty if possible.
///
/// 每 `n` 个 payload 丢弃一个，尽可能保证结果非空。
fn drop_every_nth_payload(payloads: &[Vec<u8>], n: usize) -> Vec<Vec<u8>> {
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

/// Reverse payloads within small windows of size `window`.
///
/// 在大小为 `window` 的窗口内反转 payload。
fn reverse_small_windows(payloads: &[Vec<u8>], window: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for chunk in payloads.chunks(window) {
        let mut reversed = chunk.to_vec();
        reversed.reverse();
        out.extend(reversed);
    }
    out
}

/// Truncate each payload to roughly half of its original length.
///
/// 将每个 payload 截断到原长度的一半。
fn truncate_payloads_half(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    payloads
        .iter()
        .map(|payload| {
            let take = (payload.len() / 2).max(1);
            payload[..take].to_vec()
        })
        .collect()
}

/// Swap the first two RTP payloads in the stream.
///
/// 交换流中前两个 RTP payload。
fn reorder_rtp_sequence(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = payloads.to_vec();
    let mut rtp_indexes = Vec::new();
    for (idx, payload) in out.iter().enumerate() {
        if is_rtp_packet(payload) {
            rtp_indexes.push(idx);
        }
    }
    if rtp_indexes.len() >= 2 {
        let first = rtp_indexes[0];
        let second = rtp_indexes[1];
        out.swap(first, second);
    }
    out
}

/// Check whether a payload looks like a valid RTP packet.
///
/// 检查 payload 是否像有效 RTP 包。
fn is_rtp_packet(payload: &[u8]) -> bool {
    payload.len() >= 12 && (payload[0] >> 6) == 2 && !is_rtcp_packet(payload)
}

/// Encode a list of interleaved frames (channel, payload) as an RTSP-over-TCP stream.
///
/// 将交错帧列表（通道、payload）编码为 RTSP-over-TCP 流。
fn encode_interleaved_frames(frames: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (channel, payload) in frames {
        out.push(b'$');
        out.push(*channel);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(payload);
    }
    out
}

/// Split the 4-byte interleaved header of an encoded stream into single bytes.
///
/// 将编码流中的 4 字节交错头拆成单个字节。
fn split_interleaved_header(stream: &[u8]) -> Vec<Vec<u8>> {
    if stream.len() <= 4 {
        return stream.iter().map(|byte| vec![*byte]).collect();
    }
    let mut out = stream[..4]
        .iter()
        .map(|byte| vec![*byte])
        .collect::<Vec<_>>();
    out.push(stream[4..].to_vec());
    out
}

/// Build an interleaved frame with a declared length that exceeds the actual payload.
///
/// 构造声明长度大于实际 payload 的交错帧。
fn build_interleaved_oversize_frame(frame: &(u8, Vec<u8>)) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(b'$');
    out.push(frame.0);
    let declared = frame.1.len().saturating_add(1024).min(u16::MAX as usize) as u16;
    out.extend_from_slice(&declared.to_be_bytes());
    out.extend_from_slice(&frame.1);
    out
}

/// Encode bytes as standard base64 without line breaks.
///
/// 将字节编码为标准 base64（无换行）。
fn base64_encode(input: &[u8]) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len().div_ceil(3) * 4);
    let mut index = 0usize;
    while index + 3 <= input.len() {
        let n = ((input[index] as u32) << 16)
            | ((input[index + 1] as u32) << 8)
            | input[index + 2] as u32;
        out.push(TABLE[((n >> 18) & 0x3f) as usize]);
        out.push(TABLE[((n >> 12) & 0x3f) as usize]);
        out.push(TABLE[((n >> 6) & 0x3f) as usize]);
        out.push(TABLE[(n & 0x3f) as usize]);
        index += 3;
    }

    let rem = input.len() - index;
    if rem == 1 {
        let n = (input[index] as u32) << 16;
        out.push(TABLE[((n >> 18) & 0x3f) as usize]);
        out.push(TABLE[((n >> 12) & 0x3f) as usize]);
        out.push(b'=');
        out.push(b'=');
    } else if rem == 2 {
        let n = ((input[index] as u32) << 16) | ((input[index + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 0x3f) as usize]);
        out.push(TABLE[((n >> 12) & 0x3f) as usize]);
        out.push(TABLE[((n >> 6) & 0x3f) as usize]);
        out.push(b'=');
    }
    out
}

/// Split a base64 byte stream into chunks of alternating size 1 and 3.
///
/// 将 base64 字节流按大小 1 与 3 交替切分。
fn split_http_base64_1_3(stream: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut index = 0usize;
    let mut stride = 1usize;
    while index < stream.len() {
        let take = stride.min(3).min(stream.len() - index);
        out.push(stream[index..index + take].to_vec());
        index += take;
        stride = if stride == 3 { 1 } else { stride + 1 };
    }
    out
}

/// Corrupt the first byte of a base64 stream to make it invalid.
///
/// 破坏 base64 流的第一个字节使其无效。
fn build_invalid_base64_payload(stream: &[u8]) -> Vec<u8> {
    if stream.is_empty() {
        return vec![b'#'];
    }
    let mut out = stream.to_vec();
    out[0] = b'#';
    out
}

/// Check whether a payload is a valid RTCP packet.
///
/// 检查 payload 是否为有效 RTCP 包。
fn is_rtcp_packet(payload: &[u8]) -> bool {
    payload.len() >= 2 && (payload[0] >> 6) == 2 && (200..=204).contains(&payload[1])
}

/// Verify that a record's flag field is non-zero and known.
///
/// 验证记录的 flags 字段非零且已知。
fn validate_record_flags(raw: u8) -> Result<(), CaptureFixtureError> {
    if raw == 0 || (raw & !KNOWN_RECORD_FLAGS) != 0 {
        return Err(CaptureFixtureError::InvalidRecordFlags { raw });
    }
    Ok(())
}

/// Parse the comma-separated `expect_methods` field.
///
/// 解析逗号分隔的 `expect_methods` 字段。
fn parse_expect_methods(line: usize, value: &str) -> Result<Vec<String>, CaptureFixtureError> {
    let methods: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_owned())
        .collect();
    if methods.is_empty() {
        return Err(CaptureFixtureError::InvalidMethods {
            line,
            value: value.to_owned(),
        });
    }
    Ok(methods)
}

/// Parse a numeric manifest field as `usize`.
///
/// 将数值型清单字段解析为 `usize`。
fn parse_number(
    line: usize,
    field: &'static str,
    value: &str,
) -> Result<usize, CaptureFixtureError> {
    value
        .parse::<usize>()
        .map_err(|_| CaptureFixtureError::InvalidNumber {
            line,
            field,
            value: value.to_owned(),
        })
}

/// Reject absolute, empty, or `..` fixture paths.
///
/// 拒绝绝对、空或包含 `..` 的 fixture 路径。
fn validate_fixture_path(line: usize, path: &Path) -> Result<(), CaptureFixtureError> {
    if path.is_absolute() || path.as_os_str().is_empty() {
        return Err(CaptureFixtureError::UnsafeFixturePath {
            line,
            path: path.to_path_buf(),
        });
    }

    for component in path.components() {
        if matches!(component, Component::ParentDir | Component::RootDir) {
            return Err(CaptureFixtureError::UnsafeFixturePath {
                line,
                path: path.to_path_buf(),
            });
        }
    }

    Ok(())
}

/// Read a single byte from the `rtspcap` buffer and advance the cursor.
///
/// 从 `rtspcap` 缓冲区读取一字节并推进游标。
fn read_u8(
    bytes: &[u8],
    cursor: &mut usize,
    context: &'static str,
) -> Result<u8, CaptureFixtureError> {
    if *cursor + 1 > bytes.len() {
        return Err(CaptureFixtureError::Truncated {
            context: context.to_owned(),
        });
    }
    let value = bytes[*cursor];
    *cursor += 1;
    Ok(value)
}

/// Read a big-endian `u16` from the `rtspcap` buffer and advance the cursor.
///
/// 从 `rtspcap` 缓冲区读取大端 `u16` 并推进游标。
fn read_u16(
    bytes: &[u8],
    cursor: &mut usize,
    context: &'static str,
) -> Result<u16, CaptureFixtureError> {
    let slice = read_bytes(bytes, cursor, 2, context)?;
    Ok(u16::from_be_bytes([slice[0], slice[1]]))
}

/// Read a big-endian `u32` from the `rtspcap` buffer and advance the cursor.
///
/// 从 `rtspcap` 缓冲区读取大端 `u32` 并推进游标。
fn read_u32(
    bytes: &[u8],
    cursor: &mut usize,
    context: &'static str,
) -> Result<u32, CaptureFixtureError> {
    let slice = read_bytes(bytes, cursor, 4, context)?;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Read a slice of bytes from the `rtspcap` buffer and advance the cursor.
///
/// 从 `rtspcap` 缓冲区读取一段字节并推进游标。
fn read_bytes<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    len: usize,
    context: &'static str,
) -> Result<&'a [u8], CaptureFixtureError> {
    if *cursor + len > bytes.len() {
        return Err(CaptureFixtureError::Truncated {
            context: context.to_owned(),
        });
    }
    let out = &bytes[*cursor..*cursor + len];
    *cursor += len;
    Ok(out)
}
