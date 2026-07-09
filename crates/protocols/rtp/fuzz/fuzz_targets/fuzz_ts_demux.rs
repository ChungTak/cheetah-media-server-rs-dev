#![no_main]

use cheetah_codec::{MpegTsDemuxer, MpegTsDemuxerConfig};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    // Drive the TS demuxer with arbitrary input. Should never panic regardless of payload shape.
    let mut demuxer = MpegTsDemuxer::new(MpegTsDemuxerConfig::default());
    let _ = demuxer.push(data);
    let _ = demuxer.flush();
});
