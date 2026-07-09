use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use bytes::Bytes;

pub const MANIFEST_HEADER: &str = "case\tsource_pcap\tstream_name\tmedia_sig\trole\tfixture\texpect_connected\texpect_publish\texpect_play\texpect_media_min\tnotes";
pub const MAX_FIXTURE_BYTES: u64 = 262_144;
const RTMPFLOW_MAGIC: &[u8; 4] = b"CRF1";
const MANIFEST_FIELD_COUNT: usize = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRole {
    ServerPublishC2s,
    ServerPlayC2s,
    ClientPublishS2c,
    ClientPlayS2c,
    RobustnessProbe,
}

impl CaptureRole {
    fn parse(line: usize, value: &str) -> Result<Self, CaptureFixtureError> {
        match value {
            "server_publish_c2s" => Ok(Self::ServerPublishC2s),
            "server_play_c2s" => Ok(Self::ServerPlayC2s),
            "client_publish_s2c" => Ok(Self::ClientPublishS2c),
            "client_play_s2c" => Ok(Self::ClientPlayS2c),
            "robustness_probe" => Ok(Self::RobustnessProbe),
            _ => Err(CaptureFixtureError::InvalidRole {
                line,
                value: value.to_owned(),
            }),
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
    pub role: CaptureRole,
    pub fixture: PathBuf,
    pub expect_connected: bool,
    pub expect_publish: bool,
    pub expect_play: bool,
    pub expect_media_min: usize,
    pub notes: String,
}

#[derive(Debug, Clone)]
pub struct CaptureFixture {
    pub row: CaptureManifestRow,
    pub records: Vec<Vec<u8>>,
}

impl CaptureFixture {
    pub fn is_standard_publish(&self) -> bool {
        self.row.role == CaptureRole::ServerPublishC2s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportViewKind {
    PristineRecords,
    ChunkedBytes,
    CoalescedPairs,
    PrefixTruncated,
    SuffixTruncatedRecord,
    DuplicatedRecord,
    ReorderedAdjacent,
    DroppedEveryNth,
}

impl TransportViewKind {
    pub fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::PristineRecords),
            1 => Some(Self::ChunkedBytes),
            2 => Some(Self::CoalescedPairs),
            3 => Some(Self::PrefixTruncated),
            4 => Some(Self::SuffixTruncatedRecord),
            5 => Some(Self::DuplicatedRecord),
            6 => Some(Self::ReorderedAdjacent),
            7 => Some(Self::DroppedEveryNth),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TransportView {
    pub kind: TransportViewKind,
    pub chunk_size: usize,
    pub truncation_point: usize,
    pub repeat_count: usize,
    pub drop_step: usize,
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
    InvalidFlag {
        line: usize,
        field: &'static str,
        value: String,
    },
    InvalidNumber {
        line: usize,
        field: &'static str,
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
            Self::InvalidFlag { line, field, value } => {
                write!(f, "invalid flag {field:?}={value:?} at line {line}")
            }
            Self::InvalidNumber { line, field, value } => {
                write!(f, "invalid number {field:?}={value:?} at line {line}")
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
            Self::BadMagic => write!(f, "invalid rtmpflow magic"),
            Self::Truncated { context } => write!(f, "truncated rtmpflow while reading {context}"),
            Self::ZeroLengthRecord { index } => {
                write!(f, "rtmpflow record {index} has zero length")
            }
            Self::RecordCountMismatch { expected, actual } => {
                write!(
                    f,
                    "record count mismatch: expected {expected}, got {actual}"
                )
            }
            Self::TrailingBytes { bytes } => {
                write!(f, "rtmpflow has {bytes} trailing bytes")
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
            ("role", fields[4]),
            ("fixture", fields[5]),
            ("expect_connected", fields[6]),
            ("expect_publish", fields[7]),
            ("expect_play", fields[8]),
            ("expect_media_min", fields[9]),
        ] {
            if value.is_empty() {
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
            role: CaptureRole::parse(line_number, fields[4])?,
            fixture: PathBuf::from(fields[5]),
            expect_connected: parse_flag(line_number, "expect_connected", fields[6])?,
            expect_publish: parse_flag(line_number, "expect_publish", fields[7])?,
            expect_play: parse_flag(line_number, "expect_play", fields[8])?,
            expect_media_min: fields[9].parse().map_err(|_| {
                CaptureFixtureError::InvalidNumber {
                    line: line_number,
                    field: "expect_media_min",
                    value: fields[9].to_owned(),
                }
            })?,
            notes: fields[10].to_owned(),
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
        decode_rtmpflow(&bytes)?;
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
        let records = decode_rtmpflow(&bytes)?
            .into_iter()
            .map(|record| record.to_vec())
            .collect();
        fixtures.push(CaptureFixture { row, records });
    }
    Ok(fixtures)
}

pub fn decode_rtmpflow(bytes: &[u8]) -> Result<Vec<&[u8]>, CaptureFixtureError> {
    if bytes.len() < 8 {
        return Err(CaptureFixtureError::Truncated {
            context: "header".to_owned(),
        });
    }
    if &bytes[..4] != RTMPFLOW_MAGIC {
        return Err(CaptureFixtureError::BadMagic);
    }
    let expected = u32::from_be_bytes(bytes[4..8].try_into().expect("slice length checked"));
    let mut offset = 8;
    let mut records = Vec::new();
    for index in 0..expected as usize {
        if bytes.len().saturating_sub(offset) < 4 {
            return Err(CaptureFixtureError::Truncated {
                context: format!("record {index} length"),
            });
        }
        let len = u32::from_be_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("slice length checked"),
        ) as usize;
        offset += 4;
        if len == 0 {
            return Err(CaptureFixtureError::ZeroLengthRecord { index });
        }
        if bytes.len().saturating_sub(offset) < len {
            return Err(CaptureFixtureError::Truncated {
                context: format!("record {index} payload"),
            });
        }
        records.push(&bytes[offset..offset + len]);
        offset += len;
    }
    if records.len() != expected as usize {
        return Err(CaptureFixtureError::RecordCountMismatch {
            expected,
            actual: records.len(),
        });
    }
    if offset != bytes.len() {
        return Err(CaptureFixtureError::TrailingBytes {
            bytes: bytes.len() - offset,
        });
    }
    Ok(records)
}

pub fn build_transport_view(records: &[Vec<u8>], view: TransportView) -> Vec<Bytes> {
    match view.kind {
        TransportViewKind::PristineRecords => records_to_bytes(records),
        TransportViewKind::ChunkedBytes => {
            let wire = coalesced_wire(records);
            let chunk_size = view.chunk_size.max(1);
            wire.chunks(chunk_size)
                .map(Bytes::copy_from_slice)
                .collect()
        }
        TransportViewKind::CoalescedPairs => coalesced_pairs(records),
        TransportViewKind::PrefixTruncated => prefix_truncated(records, view.truncation_point),
        TransportViewKind::SuffixTruncatedRecord => suffix_truncated_record(records),
        TransportViewKind::DuplicatedRecord => {
            duplicated_record(records, view.truncation_point, view.repeat_count)
        }
        TransportViewKind::ReorderedAdjacent => reordered_adjacent(records, view.truncation_point),
        TransportViewKind::DroppedEveryNth => dropped_every_nth(records, view.drop_step),
    }
}

fn records_to_bytes(records: &[Vec<u8>]) -> Vec<Bytes> {
    records
        .iter()
        .map(|record| Bytes::copy_from_slice(record))
        .collect()
}

fn coalesced_wire(records: &[Vec<u8>]) -> Vec<u8> {
    let total = records.iter().map(Vec::len).sum();
    let mut wire = Vec::with_capacity(total);
    for record in records {
        wire.extend_from_slice(record);
    }
    wire
}

fn coalesced_pairs(records: &[Vec<u8>]) -> Vec<Bytes> {
    let mut chunks = Vec::with_capacity(records.len().div_ceil(2));
    for pair in records.chunks(2) {
        chunks.push(Bytes::from(coalesced_wire(pair)));
    }
    chunks
}

fn prefix_truncated(records: &[Vec<u8>], truncation_point: usize) -> Vec<Bytes> {
    let wire = coalesced_wire(records);
    if wire.is_empty() {
        return Vec::new();
    }
    let keep = truncation_point.clamp(1, wire.len().saturating_sub(1).max(1));
    vec![Bytes::copy_from_slice(&wire[..keep])]
}

fn suffix_truncated_record(records: &[Vec<u8>]) -> Vec<Bytes> {
    let mut chunks = records_to_bytes(records);
    if let (Some(last_chunk), Some(last_record)) = (chunks.last_mut(), records.last()) {
        let truncated_len = last_record.len() / 2;
        *last_chunk = Bytes::copy_from_slice(&last_record[..truncated_len]);
    }
    chunks
}

fn duplicated_record(records: &[Vec<u8>], index_hint: usize, repeat_count: usize) -> Vec<Bytes> {
    let Some(index) = post_handshake_index(records.len(), index_hint) else {
        return records_to_bytes(records);
    };
    let mut chunks = records_to_bytes(records);
    let duplicate = Bytes::copy_from_slice(&records[index]);
    let repeats = repeat_count.clamp(1, 3);
    for _ in 0..repeats {
        chunks.insert(index, duplicate.clone());
    }
    chunks
}

fn reordered_adjacent(records: &[Vec<u8>], index_hint: usize) -> Vec<Bytes> {
    if records.len() <= 3 {
        return records_to_bytes(records);
    }
    let start = 2 + (index_hint % (records.len() - 3));
    let mut chunks = records_to_bytes(records);
    chunks.swap(start, start + 1);
    chunks
}

fn dropped_every_nth(records: &[Vec<u8>], drop_step: usize) -> Vec<Bytes> {
    let step = drop_step.max(2);
    let kept: Vec<Vec<u8>> = records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            if index < 2 {
                return Some(record.clone());
            }
            let post_handshake_ordinal = index - 2 + 1;
            (!post_handshake_ordinal.is_multiple_of(step)).then_some(record.clone())
        })
        .collect();
    records_to_bytes(&kept)
}

fn post_handshake_index(record_len: usize, index_hint: usize) -> Option<usize> {
    if record_len <= 2 {
        return None;
    }
    Some(2 + (index_hint % (record_len - 2)))
}

fn parse_flag(line: usize, field: &'static str, value: &str) -> Result<bool, CaptureFixtureError> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(CaptureFixtureError::InvalidFlag {
            line,
            field,
            value: value.to_owned(),
        }),
    }
}

fn validate_fixture_path(line: usize, path: &Path) -> Result<(), CaptureFixtureError> {
    if path.as_os_str().is_empty() {
        return Err(CaptureFixtureError::UnsafeFixturePath {
            line,
            path: path.to_owned(),
        });
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(CaptureFixtureError::UnsafeFixturePath {
                    line,
                    path: path.to_owned(),
                });
            }
        }
    }
    Ok(())
}
