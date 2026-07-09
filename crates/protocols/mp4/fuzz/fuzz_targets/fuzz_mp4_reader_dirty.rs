#![no_main]
use libfuzzer_sys::fuzz_target;

use bytes::Bytes;
use cheetah_codec::{Mp4ReadEvent, Mp4ReadResult, Mp4Reader, Mp4ReaderConfig};

// Drives the reader against arbitrary input that has already been chunked
// into pieces by the fuzzer. The reader is fed bytes only when it explicitly
// asks for them; we serve from a sliding cursor to exercise out-of-order and
// short-read scenarios.
fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut reader = Mp4Reader::new(Mp4ReaderConfig {
        max_box_bytes: 64 * 1024,
        max_top_level_scan: 64 * 1024,
    });
    reader.set_file_size(data.len() as u64);
    let mut steps = 0;
    loop {
        steps += 1;
        if steps > 128 {
            break;
        }
        match reader.step() {
            Mp4ReadEvent::NeedBytes(req) => {
                let end = (req.offset + req.length) as usize;
                let end = end.min(data.len());
                let start = (req.offset as usize).min(data.len());
                if start >= end {
                    break;
                }
                reader.feed_bytes(Mp4ReadResult {
                    offset: req.offset,
                    data: Bytes::copy_from_slice(&data[start..end]),
                });
            }
            Mp4ReadEvent::Eof | Mp4ReadEvent::Idle => break,
            _ => {}
        }
    }
});
