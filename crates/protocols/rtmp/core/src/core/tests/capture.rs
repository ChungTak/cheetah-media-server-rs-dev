use alloc::vec;
use alloc::vec::Vec;

use bytes::Bytes;

use super::super::{CoreInput, CoreOutput, RtmpCore, RtmpCoreCommand, RtmpEvent, RtmpMediaType};

const H264_AAC_PUBLISH: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtmp-capture/standard/h264_aac_publish.rtmpflow"
);
const H265_AAC_PUBLISH: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtmp-capture/standard/h265_aac_publish.rtmpflow"
);
const H265_LARGE_PUBLISH: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtmp-capture/standard/h265_large_publish.rtmpflow"
);
const AUDIO_ONLY_PUBLISH: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtmp-capture/standard/audio_only_publish.rtmpflow"
);
const AV1_PROBE: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtmp-capture/probes/av1_probe.rtmpflow"
);
const VP8_PROBE: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtmp-capture/probes/vp8_probe.rtmpflow"
);
const VP9_PROBE: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtmp-capture/probes/vp9_probe.rtmpflow"
);
const H266_PROBE: &[u8] = include_bytes!(
    "../../../../testing/property-tests/tests/testdata/rtmp-capture/probes/h266_probe.rtmpflow"
);
const POST_HANDSHAKE_START: usize = 2;

#[derive(Clone, Copy)]
struct CaptureCase {
    name: &'static str,
    bytes: &'static [u8],
    expect_media_min: usize,
}

#[derive(Clone, Copy)]
enum InputView {
    Records,
    Coalesced,
    SingleBytes,
}

#[derive(Clone, Copy, Debug)]
enum RobustnessView {
    CoalescedPairs,
    PrefixHalf,
    PrefixThreeQuarters,
    SuffixTruncatedRecord,
    DuplicatedPostHandshakeRecord,
    ReorderedAdjacentPostHandshakeRecords,
    DroppedEveryFifthPostHandshakeRecord,
}

const STANDARD_CASES: &[CaptureCase] = &[
    CaptureCase {
        name: "h264_aac_publish",
        bytes: H264_AAC_PUBLISH,
        expect_media_min: 1,
    },
    CaptureCase {
        name: "h265_aac_publish",
        bytes: H265_AAC_PUBLISH,
        expect_media_min: 1,
    },
    CaptureCase {
        name: "h265_large_publish",
        bytes: H265_LARGE_PUBLISH,
        expect_media_min: 1,
    },
    CaptureCase {
        name: "audio_only_publish",
        bytes: AUDIO_ONLY_PUBLISH,
        expect_media_min: 1,
    },
];

const ALL_CASES: &[CaptureCase] = &[
    CaptureCase {
        name: "h264_aac_publish",
        bytes: H264_AAC_PUBLISH,
        expect_media_min: 1,
    },
    CaptureCase {
        name: "h265_aac_publish",
        bytes: H265_AAC_PUBLISH,
        expect_media_min: 1,
    },
    CaptureCase {
        name: "h265_large_publish",
        bytes: H265_LARGE_PUBLISH,
        expect_media_min: 1,
    },
    CaptureCase {
        name: "audio_only_publish",
        bytes: AUDIO_ONLY_PUBLISH,
        expect_media_min: 1,
    },
    CaptureCase {
        name: "av1_probe",
        bytes: AV1_PROBE,
        expect_media_min: 0,
    },
    CaptureCase {
        name: "vp8_probe",
        bytes: VP8_PROBE,
        expect_media_min: 0,
    },
    CaptureCase {
        name: "vp9_probe",
        bytes: VP9_PROBE,
        expect_media_min: 0,
    },
    CaptureCase {
        name: "h266_probe",
        bytes: H266_PROBE,
        expect_media_min: 0,
    },
];

const ROBUSTNESS_VIEWS: &[RobustnessView] = &[
    RobustnessView::CoalescedPairs,
    RobustnessView::PrefixHalf,
    RobustnessView::PrefixThreeQuarters,
    RobustnessView::SuffixTruncatedRecord,
    RobustnessView::DuplicatedPostHandshakeRecord,
    RobustnessView::ReorderedAdjacentPostHandshakeRecords,
    RobustnessView::DroppedEveryFifthPostHandshakeRecord,
];

#[test]
fn standard_capture_records_replay_to_publish_and_media() {
    for case in STANDARD_CASES {
        replay_and_assert_standard(*case, InputView::Records);
    }
}

#[test]
fn standard_capture_coalesced_replay_to_publish_and_media() {
    for case in STANDARD_CASES {
        replay_and_assert_standard(*case, InputView::Coalesced);
    }
}

#[test]
fn standard_capture_single_byte_replay_to_publish_and_media() {
    for case in STANDARD_CASES {
        replay_and_assert_standard(*case, InputView::SingleBytes);
    }
}

#[test]
fn capture_transport_faults_are_bounded_for_standard_and_probe_fixtures() {
    for case in ALL_CASES {
        let records = decode_rtmpflow(case.bytes);
        for view in ROBUSTNESS_VIEWS {
            let inputs = build_robustness_view(&records, *view);
            assert!(
                inputs.len() <= records.len().saturating_mul(2).saturating_add(4),
                "{} {:?} produced an unbounded input view",
                case.name,
                view
            );
            replay_server_inputs_allowing_error(case.name, *view, inputs);
        }
    }
}

fn replay_and_assert_standard(case: CaptureCase, view: InputView) {
    let records = decode_rtmpflow(case.bytes);
    let inputs = build_input_view(&records, view);
    let events = replay_server_inputs(case.name, inputs);

    let connected = events
        .iter()
        .any(|event| matches!(event, RtmpEvent::Connected { .. }));
    let publish_requested = events
        .iter()
        .any(|event| matches!(event, RtmpEvent::PublishRequested { .. }));
    let media_count = events
        .iter()
        .filter(|event| matches!(event, RtmpEvent::MediaData { .. }))
        .count();

    assert!(connected, "{} should emit Connected", case.name);
    assert!(
        publish_requested,
        "{} should emit PublishRequested",
        case.name
    );
    assert!(
        media_count >= case.expect_media_min,
        "{} should emit at least {} media events, got {}",
        case.name,
        case.expect_media_min,
        media_count
    );
    assert_media_timestamps_monotonic(case.name, &events);
}

fn decode_rtmpflow(bytes: &'static [u8]) -> Vec<&'static [u8]> {
    assert!(bytes.len() >= 8, "rtmpflow header must be present");
    assert_eq!(&bytes[..4], b"CRF1", "rtmpflow magic");
    let expected = u32::from_be_bytes(bytes[4..8].try_into().expect("record count")) as usize;
    let mut offset = 8;
    let mut records = Vec::with_capacity(expected);
    for index in 0..expected {
        assert!(
            bytes.len().saturating_sub(offset) >= 4,
            "record {index} length must be present"
        );
        let len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("record length"))
            as usize;
        offset += 4;
        assert!(len > 0, "record {index} must not be empty");
        assert!(
            bytes.len().saturating_sub(offset) >= len,
            "record {index} payload must be present"
        );
        records.push(&bytes[offset..offset + len]);
        offset += len;
    }
    assert_eq!(offset, bytes.len(), "rtmpflow must not have trailing bytes");
    records
}

fn build_robustness_view(records: &[&[u8]], view: RobustnessView) -> Vec<Bytes> {
    match view {
        RobustnessView::CoalescedPairs => coalesced_pairs(records),
        RobustnessView::PrefixHalf => records_to_bytes(&records[..records.len() / 2]),
        RobustnessView::PrefixThreeQuarters => {
            records_to_bytes(&records[..records.len().saturating_mul(3) / 4])
        }
        RobustnessView::SuffixTruncatedRecord => suffix_truncated_record(records),
        RobustnessView::DuplicatedPostHandshakeRecord => {
            let mut mutated = records.to_vec();
            if let Some(record) = records.get(POST_HANDSHAKE_START) {
                mutated.insert(POST_HANDSHAKE_START, *record);
            }
            records_to_bytes(&mutated)
        }
        RobustnessView::ReorderedAdjacentPostHandshakeRecords => {
            let mut mutated = records.to_vec();
            if mutated.len() > POST_HANDSHAKE_START + 1 {
                mutated.swap(POST_HANDSHAKE_START, POST_HANDSHAKE_START + 1);
            }
            records_to_bytes(&mutated)
        }
        RobustnessView::DroppedEveryFifthPostHandshakeRecord => {
            let mutated: Vec<&[u8]> = records
                .iter()
                .enumerate()
                .filter_map(|(index, record)| {
                    if index < POST_HANDSHAKE_START {
                        return Some(*record);
                    }
                    let post_handshake_ordinal = index - POST_HANDSHAKE_START + 1;
                    (!post_handshake_ordinal.is_multiple_of(5)).then_some(*record)
                })
                .collect();
            records_to_bytes(&mutated)
        }
    }
}

fn records_to_bytes(records: &[&[u8]]) -> Vec<Bytes> {
    records
        .iter()
        .map(|record| Bytes::copy_from_slice(record))
        .collect()
}

fn coalesced_pairs(records: &[&[u8]]) -> Vec<Bytes> {
    let mut chunks = Vec::with_capacity(records.len().div_ceil(2));
    for pair in records.chunks(2) {
        let total = pair.iter().map(|record| record.len()).sum();
        let mut merged = Vec::with_capacity(total);
        for record in pair {
            merged.extend_from_slice(record);
        }
        chunks.push(Bytes::from(merged));
    }
    chunks
}

fn suffix_truncated_record(records: &[&[u8]]) -> Vec<Bytes> {
    let mut chunks = records_to_bytes(records);
    if let (Some(last_chunk), Some(last_record)) = (chunks.last_mut(), records.last()) {
        let truncated_len = last_record.len() / 2;
        *last_chunk = Bytes::copy_from_slice(&last_record[..truncated_len]);
    }
    chunks
}

fn build_input_view(records: &[&[u8]], view: InputView) -> Vec<Bytes> {
    match view {
        InputView::Records => records
            .iter()
            .map(|record| Bytes::copy_from_slice(record))
            .collect(),
        InputView::Coalesced => {
            let total: usize = records.iter().map(|record| record.len()).sum();
            let mut merged = Vec::with_capacity(total);
            for record in records {
                merged.extend_from_slice(record);
            }
            vec![Bytes::from(merged)]
        }
        InputView::SingleBytes => {
            let total: usize = records.iter().map(|record| record.len()).sum();
            let mut chunks = Vec::with_capacity(total);
            for record in records {
                for byte in *record {
                    chunks.push(Bytes::copy_from_slice(core::slice::from_ref(byte)));
                }
            }
            chunks
        }
    }
}

fn replay_server_inputs_allowing_error(
    _case_name: &str,
    _view: RobustnessView,
    inputs: Vec<Bytes>,
) {
    let mut core = RtmpCore::new();
    let mut publish_accepted = false;

    for input in inputs {
        let outputs = match core.handle_input(CoreInput::Bytes(input)) {
            Ok(outputs) => outputs,
            Err(_) => break,
        };
        if !accept_publish_from_outputs_allowing_error(&mut core, outputs, &mut publish_accepted) {
            break;
        }
    }
}

fn accept_publish_from_outputs_allowing_error(
    core: &mut RtmpCore,
    outputs: Vec<CoreOutput>,
    publish_accepted: &mut bool,
) -> bool {
    for output in outputs {
        let CoreOutput::Event(RtmpEvent::PublishRequested { stream_id, .. }) = output else {
            continue;
        };
        if *publish_accepted {
            continue;
        }
        *publish_accepted = true;
        if core
            .handle_input(CoreInput::Command(RtmpCoreCommand::AcceptPublish {
                stream_id,
            }))
            .is_err()
        {
            return false;
        }
    }
    true
}

fn replay_server_inputs(case_name: &str, inputs: Vec<Bytes>) -> Vec<RtmpEvent> {
    let mut core = RtmpCore::new();
    let mut events = Vec::new();
    let mut publish_accepted = false;

    for input in inputs {
        let outputs = core
            .handle_input(CoreInput::Bytes(input))
            .unwrap_or_else(|err| panic!("{case_name} replay failed: {err}"));
        collect_events_and_accept_publish(
            case_name,
            &mut core,
            outputs,
            &mut events,
            &mut publish_accepted,
        );
    }

    events
}

fn collect_events_and_accept_publish(
    case_name: &str,
    core: &mut RtmpCore,
    outputs: Vec<CoreOutput>,
    events: &mut Vec<RtmpEvent>,
    publish_accepted: &mut bool,
) {
    for output in outputs {
        let CoreOutput::Event(event) = output else {
            continue;
        };
        if let RtmpEvent::PublishRequested { stream_id, .. } = &event {
            let stream_id = *stream_id;
            events.push(event);
            if !*publish_accepted {
                *publish_accepted = true;
                let accept_outputs = core
                    .handle_input(CoreInput::Command(RtmpCoreCommand::AcceptPublish {
                        stream_id,
                    }))
                    .unwrap_or_else(|err| panic!("{case_name} AcceptPublish failed: {err}"));
                collect_events_and_accept_publish(
                    case_name,
                    core,
                    accept_outputs,
                    events,
                    publish_accepted,
                );
            }
        } else {
            events.push(event);
        }
    }
}

fn assert_media_timestamps_monotonic(case_name: &str, events: &[RtmpEvent]) {
    let mut last_audio = None;
    let mut last_video = None;
    let mut last_data = None;

    for event in events {
        let RtmpEvent::MediaData {
            timestamp_ms,
            media_type,
            ..
        } = event
        else {
            continue;
        };
        let last = match media_type {
            RtmpMediaType::Audio => &mut last_audio,
            RtmpMediaType::Video => &mut last_video,
            RtmpMediaType::Data => &mut last_data,
        };
        if let Some(previous) = *last {
            assert!(
                previous <= *timestamp_ms,
                "{} {:?} timestamp moved backward: {} -> {}",
                case_name,
                media_type,
                previous,
                timestamp_ms
            );
        }
        *last = Some(*timestamp_ms);
    }
}
