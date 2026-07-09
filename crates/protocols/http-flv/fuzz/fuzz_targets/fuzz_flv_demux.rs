#![no_main]

use cheetah_codec::FlvDemuxer;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut demuxer = FlvDemuxer::new(4 * 1024 * 1024);
    if data.is_empty() {
        return;
    }

    let split = (data[0] as usize).saturating_add(1);
    for chunk in data[1..].chunks(split) {
        let _ = demuxer.push(chunk);
    }
});
