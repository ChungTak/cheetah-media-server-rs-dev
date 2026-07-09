//! Replay real RTMP C2S capture fixtures through the Sans-I/O server core.

#![no_main]

mod common;

use cheetah_rtmp_core::RtmpCore;
use common::{
    build_server_capture_view, capture_records_from_data_or_seed, feed_server_chunks_with_accept,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (_case_name, records) = capture_records_from_data_or_seed(data);
    if records.is_empty() {
        return;
    }
    let view_selector = data.get(1).copied().unwrap_or_default();
    let chunk_hint = data.get(2).copied().unwrap_or_default();
    let chunks = build_server_capture_view(&records, view_selector, chunk_hint);

    let mut core = RtmpCore::new();
    feed_server_chunks_with_accept(&mut core, chunks);
});
