#![no_main]

use cheetah_codec::{PsDemuxer, PsDemuxerConfig};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());
    let _ = demuxer.push(data);
});
