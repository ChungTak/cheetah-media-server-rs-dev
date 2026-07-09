#[derive(Debug, Clone)]
pub struct CapturePublishCase {
    pub name: &'static str,
    pub bytes: &'static [u8],
    pub expect_video: bool,
    pub expect_audio: bool,
}

impl CapturePublishCase {
    pub fn records(&self) -> Vec<&'static [u8]> {
        decode_rtmpflow(self.bytes).unwrap_or_else(|err| {
            panic!(
                "invalid committed RTMP capture fixture {}: {err}",
                self.name
            )
        })
    }
}

pub fn standard_publish_cases() -> Vec<CapturePublishCase> {
    vec![
        CapturePublishCase {
            name: "h264_aac_publish",
            expect_video: true,
            expect_audio: true,
            bytes: include_bytes!(
                "../../../testing/property-tests/tests/testdata/rtmp-capture/standard/h264_aac_publish.rtmpflow"
            ),
        },
        CapturePublishCase {
            name: "h265_aac_publish",
            expect_video: true,
            expect_audio: true,
            bytes: include_bytes!(
                "../../../testing/property-tests/tests/testdata/rtmp-capture/standard/h265_aac_publish.rtmpflow"
            ),
        },
        CapturePublishCase {
            name: "h265_large_publish",
            expect_video: true,
            expect_audio: true,
            bytes: include_bytes!(
                "../../../testing/property-tests/tests/testdata/rtmp-capture/standard/h265_large_publish.rtmpflow"
            ),
        },
        CapturePublishCase {
            name: "audio_only_publish",
            expect_video: false,
            expect_audio: true,
            bytes: include_bytes!(
                "../../../testing/property-tests/tests/testdata/rtmp-capture/standard/audio_only_publish.rtmpflow"
            ),
        },
    ]
}

pub fn play_acceptance_cases() -> Vec<CapturePublishCase> {
    standard_publish_cases()
        .into_iter()
        .filter(|case| {
            matches!(
                case.name,
                "h264_aac_publish" | "h265_aac_publish" | "audio_only_publish"
            )
        })
        .collect()
}

pub fn probe_publish_cases() -> Vec<CapturePublishCase> {
    vec![
        CapturePublishCase {
            name: "av1_probe",
            expect_video: false,
            expect_audio: false,
            bytes: include_bytes!(
                "../../../testing/property-tests/tests/testdata/rtmp-capture/probes/av1_probe.rtmpflow"
            ),
        },
        CapturePublishCase {
            name: "vp8_probe",
            expect_video: false,
            expect_audio: false,
            bytes: include_bytes!(
                "../../../testing/property-tests/tests/testdata/rtmp-capture/probes/vp8_probe.rtmpflow"
            ),
        },
        CapturePublishCase {
            name: "vp9_probe",
            expect_video: false,
            expect_audio: false,
            bytes: include_bytes!(
                "../../../testing/property-tests/tests/testdata/rtmp-capture/probes/vp9_probe.rtmpflow"
            ),
        },
        CapturePublishCase {
            name: "h266_probe",
            expect_video: false,
            expect_audio: false,
            bytes: include_bytes!(
                "../../../testing/property-tests/tests/testdata/rtmp-capture/probes/h266_probe.rtmpflow"
            ),
        },
    ]
}

pub fn module_health_fault_cases() -> Vec<CapturePublishCase> {
    let mut cases = standard_publish_cases();
    cases.extend(probe_publish_cases());
    cases
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFaultKind {
    PrefixTruncated,
    DroppedEveryNth,
    ReorderedAdjacent,
}

pub fn build_fault_chunks(case: &CapturePublishCase, fault: CaptureFaultKind) -> Vec<Vec<u8>> {
    let records = case.records();
    match fault {
        CaptureFaultKind::PrefixTruncated => {
            let wire = coalesced_wire(&records);
            if wire.len() <= 1 {
                return vec![wire];
            }
            vec![wire[..wire.len() / 2].to_vec()]
        }
        CaptureFaultKind::DroppedEveryNth => records
            .into_iter()
            .enumerate()
            .filter_map(|(index, record)| {
                if index < 2 || !(index - 1).is_multiple_of(5) {
                    Some(record.to_vec())
                } else {
                    None
                }
            })
            .collect(),
        CaptureFaultKind::ReorderedAdjacent => {
            let mut chunks: Vec<Vec<u8>> =
                records.into_iter().map(|record| record.to_vec()).collect();
            if chunks.len() > 5 {
                chunks.swap(4, 5);
            }
            chunks
        }
    }
}

fn coalesced_wire(records: &[&[u8]]) -> Vec<u8> {
    let total = records.iter().map(|record| record.len()).sum();
    let mut out = Vec::with_capacity(total);
    for record in records {
        out.extend_from_slice(record);
    }
    out
}

fn decode_rtmpflow(bytes: &'static [u8]) -> Result<Vec<&'static [u8]>, String> {
    const HEADER_LEN: usize = 8;
    if bytes.len() < HEADER_LEN {
        return Err(format!("fixture too short: {} bytes", bytes.len()));
    }
    if &bytes[..4] != b"CRF1" {
        return Err("missing CRF1 magic".to_string());
    }

    let record_count = u32::from_be_bytes(bytes[4..8].try_into().expect("count width")) as usize;
    let mut offset = HEADER_LEN;
    let mut records = Vec::with_capacity(record_count);
    for index in 0..record_count {
        if bytes.len().saturating_sub(offset) < 4 {
            return Err(format!("record {index} missing length field"));
        }
        let len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("length width"))
            as usize;
        offset += 4;
        if len == 0 {
            return Err(format!("record {index} has zero length"));
        }
        let end = offset
            .checked_add(len)
            .ok_or_else(|| format!("record {index} length overflows usize"))?;
        if end > bytes.len() {
            return Err(format!(
                "record {index} length {len} exceeds remaining {} bytes",
                bytes.len().saturating_sub(offset)
            ));
        }
        records.push(&bytes[offset..end]);
        offset = end;
    }

    if offset != bytes.len() {
        return Err(format!(
            "fixture has {} trailing bytes after {record_count} records",
            bytes.len() - offset
        ));
    }
    Ok(records)
}
