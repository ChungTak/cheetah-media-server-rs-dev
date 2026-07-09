#![no_main]

use cheetah_codec::{Jtt1078FrameAssembler, Jtt1078Header};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    // Standalone header parse must not panic on arbitrary bytes.
    let _ = Jtt1078Header::parse(data);
    let _ = Jtt1078Header::parse_v2019(data);

    // Frame assembly with bounded cache must also tolerate any payload shape.
    let mut assembler = Jtt1078FrameAssembler::new(64 * 1024);
    let _result = assembler.push(data);
});
