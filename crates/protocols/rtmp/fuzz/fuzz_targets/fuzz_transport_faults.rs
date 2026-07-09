//! Real RTMP capture replay with TCP-like and datagram-like transport faults.

#![no_main]

mod common;

use cheetah_rtmp_core::RtmpCore;
use common::{
    build_transport_view, capture_records_from_data_or_seed, feed_server_chunks_with_accept,
    CaptureViewKind,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (_case_name, records) = capture_records_from_data_or_seed(data);
    if records.is_empty() {
        return;
    }
    let mode = CaptureViewKind::from_selector(data.get(1).copied().unwrap_or_default());
    let parameter = data.get(2).copied().unwrap_or_default();
    let chunks = build_transport_view(&records, mode, parameter);

    let mut core = RtmpCore::new();
    feed_server_chunks_with_accept(&mut core, chunks);
});
