use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub const MANIFEST_HEADER: &str = "case\tsource_pcap\tstream_name\tmedia_sig\tpush_transport\tpull_transport\trole\tfixture\texpect_methods\texpect_rtp_min\texpect_rtcp_min\texpect_tracks_min\tnotes";
pub const MAX_FIXTURE_BYTES: u64 = 524_288;
const RTSPCAP_MAGIC: &[u8; 4] = b"RSF1";
const MANIFEST_FIELD_COUNT: usize = 13;
const KNOWN_RECORD_FLAGS: u8 = 0x01 | 0x02 | 0x04;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRecord {
    pub kind: CaptureRecordKind,
    pub flags: u8,
    pub flow_id: u16,
    pub delta_us: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CaptureFixture {
    pub row: CaptureManifestRow,
    pub records: Vec<CaptureRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedPayloadView {
    pub name: &'static str,
    pub payloads: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFaultViewError {
    InvalidConfig { view: &'static str, detail: String },
    EmptyInput { view: &'static str },
}

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
            Self::InvalidRecordKind { raw } => {
                write!(f, "invalid rtspcap record kind {raw}")
            }
            Self::InvalidRecordFlags { raw } => {
                write!(f, "invalid rtspcap record flags 0x{raw:02x}")
            }
            Self::RecordCountMismatch { expected, actual } => {
                write!(
                    f,
                    "record count mismatch: expected {expected}, got {actual}"
                )
            }
            Self::TrailingBytes { bytes } => {
                write!(f, "rtspcap has {bytes} trailing bytes")
            }
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

fn split_one_byte_chunks(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    payloads
        .iter()
        .flat_map(|payload| payload.iter().map(|byte| vec![*byte]))
        .collect()
}

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

fn total_payload_len(payloads: &[Vec<u8>]) -> usize {
    payloads.iter().map(Vec::len).sum()
}

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

fn duplicate_first_payload(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    if payloads.is_empty() {
        return Vec::new();
    }
    let mut out = payloads.to_vec();
    out.insert(1, payloads[0].clone());
    out
}

fn swap_adjacent(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = payloads.to_vec();
    if out.len() >= 2 {
        out.swap(0, 1);
    }
    out
}

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

fn reverse_small_windows(payloads: &[Vec<u8>], window: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for chunk in payloads.chunks(window) {
        let mut reversed = chunk.to_vec();
        reversed.reverse();
        out.extend(reversed);
    }
    out
}

fn truncate_payloads_half(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    payloads
        .iter()
        .map(|payload| {
            let take = (payload.len() / 2).max(1);
            payload[..take].to_vec()
        })
        .collect()
}

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

fn is_rtp_packet(payload: &[u8]) -> bool {
    payload.len() >= 12 && (payload[0] >> 6) == 2 && !is_rtcp_packet(payload)
}

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

fn build_interleaved_oversize_frame(frame: &(u8, Vec<u8>)) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(b'$');
    out.push(frame.0);
    let declared = frame.1.len().saturating_add(1024).min(u16::MAX as usize) as u16;
    out.extend_from_slice(&declared.to_be_bytes());
    out.extend_from_slice(&frame.1);
    out
}

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

fn build_invalid_base64_payload(stream: &[u8]) -> Vec<u8> {
    if stream.is_empty() {
        return vec![b'#'];
    }
    let mut out = stream.to_vec();
    out[0] = b'#';
    out
}

fn is_rtcp_packet(payload: &[u8]) -> bool {
    payload.len() >= 2 && (payload[0] >> 6) == 2 && (200..=204).contains(&payload[1])
}

fn validate_record_flags(raw: u8) -> Result<(), CaptureFixtureError> {
    if raw == 0 || (raw & !KNOWN_RECORD_FLAGS) != 0 {
        return Err(CaptureFixtureError::InvalidRecordFlags { raw });
    }
    Ok(())
}

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

fn read_u16(
    bytes: &[u8],
    cursor: &mut usize,
    context: &'static str,
) -> Result<u16, CaptureFixtureError> {
    let slice = read_bytes(bytes, cursor, 2, context)?;
    Ok(u16::from_be_bytes([slice[0], slice[1]]))
}

fn read_u32(
    bytes: &[u8],
    cursor: &mut usize,
    context: &'static str,
) -> Result<u32, CaptureFixtureError> {
    let slice = read_bytes(bytes, cursor, 4, context)?;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

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
