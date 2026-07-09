#![allow(dead_code)]

use bytes::Bytes;
use cheetah_rtmp_core::{
    RtmpChunk, RtmpChunkEncoder, RtmpChunkSize, RtmpChunkStreamId, RtmpMessageStreamId,
    RtmpMessageType, RtmpTimestamp,
};
use cheetah_rtmp_core::{CoreInput, CoreOutput, RtmpCore, RtmpCoreCommand, RtmpEvent};

const RTMPFLOW_MAGIC: &[u8; 4] = b"CRF1";
const MAX_REPLAY_RECORDS: usize = 96;
const MAX_REPLAY_BYTES: usize = 192 * 1024;
const SERVER_HANDSHAKE_WRITE_MIN: usize = 1537;

pub struct CaptureSeed {
    pub name: &'static str,
    pub bytes: &'static [u8],
}

pub const SERVER_CAPTURE_SEEDS: &[CaptureSeed] = &[
    CaptureSeed {
        name: "h264_aac_publish",
        bytes: include_bytes!(
            "../../testing/property-tests/tests/testdata/rtmp-capture/standard/h264_aac_publish.rtmpflow"
        ),
    },
    CaptureSeed {
        name: "h265_aac_publish",
        bytes: include_bytes!(
            "../../testing/property-tests/tests/testdata/rtmp-capture/standard/h265_aac_publish.rtmpflow"
        ),
    },
    CaptureSeed {
        name: "h265_large_publish",
        bytes: include_bytes!(
            "../../testing/property-tests/tests/testdata/rtmp-capture/standard/h265_large_publish.rtmpflow"
        ),
    },
    CaptureSeed {
        name: "audio_only_publish",
        bytes: include_bytes!(
            "../../testing/property-tests/tests/testdata/rtmp-capture/standard/audio_only_publish.rtmpflow"
        ),
    },
    CaptureSeed {
        name: "av1_probe",
        bytes: include_bytes!(
            "../../testing/property-tests/tests/testdata/rtmp-capture/probes/av1_probe.rtmpflow"
        ),
    },
    CaptureSeed {
        name: "vp8_probe",
        bytes: include_bytes!(
            "../../testing/property-tests/tests/testdata/rtmp-capture/probes/vp8_probe.rtmpflow"
        ),
    },
    CaptureSeed {
        name: "vp9_probe",
        bytes: include_bytes!(
            "../../testing/property-tests/tests/testdata/rtmp-capture/probes/vp9_probe.rtmpflow"
        ),
    },
    CaptureSeed {
        name: "h266_probe",
        bytes: include_bytes!(
            "../../testing/property-tests/tests/testdata/rtmp-capture/probes/h266_probe.rtmpflow"
        ),
    },
];

#[derive(Debug, Clone, Copy)]
pub enum RtmpFlowDecodeError {
    Truncated,
    BadMagic,
    EmptyRecord,
    TrailingBytes,
}

#[derive(Debug, Clone, Copy)]
pub struct FeedBounds {
    pub max_records: usize,
    pub max_bytes: usize,
}

impl Default for FeedBounds {
    fn default() -> Self {
        Self {
            max_records: MAX_REPLAY_RECORDS,
            max_bytes: MAX_REPLAY_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CaptureViewKind {
    OriginalRecords,
    SingleBuffer,
    OneByteChunks,
    CoalescedN,
    TruncatedPrefix,
    DuplicateRecord,
    SwapAdjacent,
    DropEveryNth,
}

impl CaptureViewKind {
    pub fn from_selector(selector: u8) -> Self {
        match selector % 8 {
            0 => Self::OriginalRecords,
            1 => Self::SingleBuffer,
            2 => Self::OneByteChunks,
            3 => Self::CoalescedN,
            4 => Self::TruncatedPrefix,
            5 => Self::DuplicateRecord,
            6 => Self::SwapAdjacent,
            _ => Self::DropEveryNth,
        }
    }
}

pub fn select_server_seed(data: &[u8]) -> &'static CaptureSeed {
    let index = data.first().copied().unwrap_or_default() as usize % SERVER_CAPTURE_SEEDS.len();
    &SERVER_CAPTURE_SEEDS[index]
}

pub fn capture_records_from_data_or_seed(data: &[u8]) -> (&'static str, Vec<&[u8]>) {
    let bounds = FeedBounds::default();
    if let Ok(records) = decode_rtmpflow_bounded(data, bounds) {
        return ("fuzz_input_rtmpflow", records);
    }

    let seed = select_server_seed(data);
    let Ok(records) = decode_rtmpflow(seed.bytes) else {
        return (seed.name, Vec::new());
    };
    (seed.name, bounded_records(&records, FeedBounds::default()))
}

pub fn decode_rtmpflow(bytes: &[u8]) -> Result<Vec<&[u8]>, RtmpFlowDecodeError> {
    if bytes.len() < 8 {
        return Err(RtmpFlowDecodeError::Truncated);
    }
    if &bytes[..4] != RTMPFLOW_MAGIC {
        return Err(RtmpFlowDecodeError::BadMagic);
    }

    let expected = u32::from_be_bytes(bytes[4..8].try_into().expect("slice length checked"));
    let mut records = Vec::new();
    let mut offset = 8;
    for _ in 0..expected {
        if bytes.len().saturating_sub(offset) < 4 {
            return Err(RtmpFlowDecodeError::Truncated);
        }
        let len = u32::from_be_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("slice length checked"),
        ) as usize;
        offset += 4;
        if len == 0 {
            return Err(RtmpFlowDecodeError::EmptyRecord);
        }
        if bytes.len().saturating_sub(offset) < len {
            return Err(RtmpFlowDecodeError::Truncated);
        }
        records.push(&bytes[offset..offset + len]);
        offset += len;
    }

    if offset != bytes.len() {
        return Err(RtmpFlowDecodeError::TrailingBytes);
    }
    Ok(records)
}

fn decode_rtmpflow_bounded(
    bytes: &[u8],
    bounds: FeedBounds,
) -> Result<Vec<&[u8]>, RtmpFlowDecodeError> {
    if bytes.len() < 8 {
        return Err(RtmpFlowDecodeError::Truncated);
    }
    if &bytes[..4] != RTMPFLOW_MAGIC {
        return Err(RtmpFlowDecodeError::BadMagic);
    }

    let expected = u32::from_be_bytes(bytes[4..8].try_into().expect("slice length checked"));
    let mut records = Vec::with_capacity(bounds.max_records.min(16));
    let mut total = 0usize;
    let mut offset = 8usize;
    let mut stopped_early = false;

    for _ in 0..expected {
        if records.len() >= bounds.max_records || total >= bounds.max_bytes {
            stopped_early = true;
            break;
        }
        if bytes.len().saturating_sub(offset) < 4 {
            return Err(RtmpFlowDecodeError::Truncated);
        }
        let len = u32::from_be_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("slice length checked"),
        ) as usize;
        offset += 4;
        if len == 0 {
            return Err(RtmpFlowDecodeError::EmptyRecord);
        }
        if bytes.len().saturating_sub(offset) < len {
            return Err(RtmpFlowDecodeError::Truncated);
        }

        if total.saturating_add(len) > bounds.max_bytes {
            if total == 0 && bounds.max_bytes > 0 {
                let keep = bounds.max_bytes.min(len);
                records.push(&bytes[offset..offset + keep]);
            }
            stopped_early = true;
            break;
        }

        records.push(&bytes[offset..offset + len]);
        total += len;
        offset += len;
    }

    if !stopped_early && offset != bytes.len() {
        return Err(RtmpFlowDecodeError::TrailingBytes);
    }
    Ok(records)
}

pub fn bounded_records<'a>(records: &[&'a [u8]], bounds: FeedBounds) -> Vec<&'a [u8]> {
    let mut selected = Vec::new();
    let mut total = 0usize;
    for record in records.iter().copied().take(bounds.max_records) {
        if total.saturating_add(record.len()) > bounds.max_bytes {
            if total == 0 && bounds.max_bytes > 0 {
                selected.push(&record[..bounds.max_bytes.min(record.len())]);
            }
            break;
        }
        total += record.len();
        selected.push(record);
    }
    selected
}

pub fn build_server_capture_view(records: &[&[u8]], selector: u8, chunk_hint: u8) -> Vec<Bytes> {
    let mode = match selector % 3 {
        0 => CaptureViewKind::OriginalRecords,
        1 => CaptureViewKind::SingleBuffer,
        _ => CaptureViewKind::CoalescedN,
    };
    build_transport_view(records, mode, chunk_hint)
}

pub fn build_transport_view(
    records: &[&[u8]],
    mode: CaptureViewKind,
    parameter: u8,
) -> Vec<Bytes> {
    match mode {
        CaptureViewKind::OriginalRecords => records_to_bytes(records),
        CaptureViewKind::SingleBuffer => {
            let wire = coalesced_wire(records);
            bytes_from_non_empty(wire)
        }
        CaptureViewKind::OneByteChunks => coalesced_wire(records)
            .into_iter()
            .map(|byte| Bytes::from(vec![byte]))
            .collect(),
        CaptureViewKind::CoalescedN => {
            let group = (parameter as usize % 8) + 2;
            coalesced_groups(records, group)
        }
        CaptureViewKind::TruncatedPrefix => truncated_prefix(records, parameter),
        CaptureViewKind::DuplicateRecord => duplicate_record(records, parameter),
        CaptureViewKind::SwapAdjacent => swap_adjacent(records, parameter),
        CaptureViewKind::DropEveryNth => drop_every_nth(records, parameter),
    }
}

pub fn feed_server_chunks_with_accept(core: &mut RtmpCore, chunks: Vec<Bytes>) {
    let mut publish_accepted = false;
    let mut ignored_writes = Vec::new();
    for chunk in chunks {
        let Ok(outputs) = core.handle_input(CoreInput::Bytes(chunk)) else {
            return;
        };
        if !handle_server_outputs(core, outputs, &mut publish_accepted, &mut ignored_writes) {
            return;
        }
    }
}

pub fn derive_server_post_handshake_writes(records: &[&[u8]], selector: u8, chunk_hint: u8) -> Vec<Bytes> {
    let mut core = RtmpCore::new();
    let mut publish_accepted = false;
    let mut writes = Vec::new();
    for chunk in build_server_capture_view(records, selector, chunk_hint) {
        let Ok(outputs) = core.handle_input(CoreInput::Bytes(chunk)) else {
            break;
        };
        if !handle_server_outputs(&mut core, outputs, &mut publish_accepted, &mut writes) {
            break;
        }
    }
    writes
}

fn handle_server_outputs(
    core: &mut RtmpCore,
    outputs: Vec<CoreOutput>,
    publish_accepted: &mut bool,
    post_handshake_writes: &mut Vec<Bytes>,
) -> bool {
    for output in outputs {
        match output {
            CoreOutput::Write(bytes) => {
                if bytes.len() < SERVER_HANDSHAKE_WRITE_MIN {
                    post_handshake_writes.push(bytes);
                }
            }
            CoreOutput::Event(RtmpEvent::PublishRequested { stream_id, .. }) => {
                if *publish_accepted {
                    continue;
                }
                *publish_accepted = true;
                let Ok(outputs) =
                    core.handle_input(CoreInput::Command(RtmpCoreCommand::AcceptPublish {
                        stream_id,
                    }))
                else {
                    return false;
                };
                if !handle_server_outputs(
                    core,
                    outputs,
                    publish_accepted,
                    post_handshake_writes,
                ) {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

fn records_to_bytes(records: &[&[u8]]) -> Vec<Bytes> {
    records.iter().map(|record| Bytes::copy_from_slice(record)).collect()
}

fn coalesced_wire(records: &[&[u8]]) -> Vec<u8> {
    let total = records.iter().map(|record| record.len()).sum();
    let mut wire = Vec::with_capacity(total);
    for record in records {
        wire.extend_from_slice(record);
    }
    wire
}

fn coalesced_groups(records: &[&[u8]], group: usize) -> Vec<Bytes> {
    let mut chunks = Vec::with_capacity(records.len().div_ceil(group));
    for group_records in records.chunks(group) {
        chunks.extend(bytes_from_non_empty(coalesced_wire(group_records)));
    }
    chunks
}

fn truncated_prefix(records: &[&[u8]], parameter: u8) -> Vec<Bytes> {
    let wire = coalesced_wire(records);
    if wire.is_empty() {
        return Vec::new();
    }
    let divisor = (parameter as usize % 5) + 2;
    let keep = (wire.len() / divisor).clamp(1, wire.len());
    vec![Bytes::copy_from_slice(&wire[..keep])]
}

fn duplicate_record(records: &[&[u8]], parameter: u8) -> Vec<Bytes> {
    let Some(index) = post_handshake_index(records.len(), parameter) else {
        return records_to_bytes(records);
    };
    let mut chunks = records_to_bytes(records);
    chunks.insert(index, Bytes::copy_from_slice(records[index]));
    chunks
}

fn swap_adjacent(records: &[&[u8]], parameter: u8) -> Vec<Bytes> {
    let Some(index) = post_handshake_index(records.len().saturating_sub(1), parameter) else {
        return records_to_bytes(records);
    };
    let mut chunks = records_to_bytes(records);
    chunks.swap(index, index + 1);
    chunks
}

fn drop_every_nth(records: &[&[u8]], parameter: u8) -> Vec<Bytes> {
    let step = (parameter as usize % 6) + 2;
    let kept: Vec<&[u8]> = records
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(index, record)| {
            if index < 2 {
                return Some(record);
            }
            let post_handshake_ordinal = index - 1;
            (!post_handshake_ordinal.is_multiple_of(step)).then_some(record)
        })
        .collect();
    records_to_bytes(&kept)
}

fn post_handshake_index(record_len: usize, parameter: u8) -> Option<usize> {
    if record_len <= 2 {
        return None;
    }
    Some(2 + (parameter as usize % (record_len - 2)))
}

fn bytes_from_non_empty(wire: Vec<u8>) -> Vec<Bytes> {
    if wire.is_empty() {
        Vec::new()
    } else {
        vec![Bytes::from(wire)]
    }
}

pub fn ready_core() -> RtmpCore {
    let mut core = RtmpCore::new();
    let mut c0c1 = vec![0u8; 1537];
    c0c1[0] = 3;
    let _ = core.handle_input(CoreInput::Bytes(Bytes::from(c0c1)));
    let _ = core.handle_input(CoreInput::Bytes(Bytes::from(vec![0u8; 1536])));
    core
}

pub fn feed_message(core: &mut RtmpCore, message_type: u8, message_stream_id: u32, payload: &[u8]) {
    if let Ok(message_type) = RtmpMessageType::from_type_id(message_type) {
        let chunk = RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(3).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::new(message_stream_id),
            message_type,
            timestamp: RtmpTimestamp::from_millis(0),
            payload: Bytes::copy_from_slice(payload),
        };
        let mut encoder = RtmpChunkEncoder::default();
        encoder.set_chunk_size(RtmpChunkSize::saturating_new(128));
        let mut wire = Vec::new();
        encoder.encode(&mut wire, &chunk);
        let _ = core.handle_input(CoreInput::Bytes(Bytes::from(wire)));
    }
}

pub fn feed_random_command(core: &mut RtmpCore, data: &[u8]) {
    let stream_id = read_u32(data, 1).max(1);
    let timestamp_ms = read_u32(data, 5);
    let payload = if data.len() > 9 {
        Bytes::copy_from_slice(&data[9..])
    } else {
        Bytes::new()
    };
    let description = String::from_utf8_lossy(payload.as_ref()).into_owned();

    let cmd = match data.first().copied().unwrap_or_default() % 10 {
        0 => RtmpCoreCommand::AcceptPublish { stream_id },
        1 => RtmpCoreCommand::RejectPublish {
            stream_id,
            description,
        },
        2 => RtmpCoreCommand::AcceptPlay { stream_id },
        3 => RtmpCoreCommand::AcceptPlayConfigured {
            stream_id,
            emit_play_status: data.get(2).copied().unwrap_or_default() & 0x1 == 1,
            emit_sample_access: data.get(3).copied().unwrap_or_default() & 0x1 == 1,
        },
        4 => RtmpCoreCommand::RejectPlay {
            stream_id,
            description,
        },
        5 => RtmpCoreCommand::SendMetadata {
            stream_id,
            timestamp_ms,
            payload,
        },
        6 => RtmpCoreCommand::SendAudio {
            stream_id,
            timestamp_ms,
            payload,
        },
        7 => RtmpCoreCommand::SendVideo {
            stream_id,
            timestamp_ms,
            payload,
        },
        8 => RtmpCoreCommand::CloseStream { stream_id },
        _ => RtmpCoreCommand::CloseConnection,
    };

    let _ = core.handle_input(CoreInput::Command(cmd));
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    if data.len() < offset + 4 {
        return 0;
    }
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}
