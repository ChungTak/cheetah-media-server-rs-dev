#![no_main]
use libfuzzer_sys::fuzz_target;

use cheetah_codec::{Fmp4Demuxer, Fmp4DemuxerConfig};

fuzz_target!(|data: &[u8]| {
    let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig {
        max_box_bytes: 1024 * 1024,
    });
    let _ = demuxer.push(data);
    let _ = demuxer.flush();
});
