#![no_main]
use libfuzzer_sys::fuzz_target;

use bytes::Bytes;
use cheetah_codec::{Mp4ReadEvent, Mp4ReadResult, Mp4Reader, Mp4ReaderConfig};

// Drives the Sans-I/O `Mp4Reader` against arbitrary bytes for at most a
// bounded number of iterations. The reader must never panic, never request
// reads outside the file boundary, and must converge on either `Eof`,
// `Idle`, or a fatal diagnostic state within a finite step budget.
fuzz_target!(|data: &[u8]| {
    let mut reader = Mp4Reader::new(Mp4ReaderConfig {
        max_box_bytes: 64 * 1024,
        max_top_level_scan: 64 * 1024,
    });
    reader.set_file_size(data.len() as u64);
    let mut steps = 0usize;
    while steps < 64 {
        steps += 1;
        match reader.step() {
            Mp4ReadEvent::NeedBytes(req) => {
                let end = (req.offset + req.length) as usize;
                let end = end.min(data.len());
                let start = (req.offset as usize).min(data.len());
                let slice = &data[start..end];
                reader.feed_bytes(Mp4ReadResult {
                    offset: req.offset,
                    data: Bytes::copy_from_slice(slice),
                });
            }
            Mp4ReadEvent::Tracks(_) | Mp4ReadEvent::Frame(_) | Mp4ReadEvent::Diagnostic(_) => {}
            Mp4ReadEvent::Eof | Mp4ReadEvent::Idle => break,
        }
    }
});
