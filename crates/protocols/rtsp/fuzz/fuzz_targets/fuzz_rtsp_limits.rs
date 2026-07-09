#![no_main]

mod common;

use cheetah_rtsp_core::RtspMessageLimits;
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    let bounded = common::bounded_bytes_payload(data, MAX_INPUT_BYTES);
    let limits = RtspMessageLimits {
        max_buffer_size: 4096,
        max_headers_count: 10,
        max_header_line_size: 256,
        max_body_size: 1024,
        max_interleaved_frame_size: 2048,
        validate_version: true,
    };

    common::fuzz_core_entry(&bounded, limits.clone(), 3);
    common::fuzz_message_decoders(&bounded, limits.clone(), 3);

    let mixed =
        common::build_mixed_rtsp_interleaved_input(&bounded, limits.max_interleaved_frame_size);
    common::fuzz_core_entry(&mixed, limits.clone(), 5);
    common::fuzz_message_decoders(&mixed, limits, 7);
});
