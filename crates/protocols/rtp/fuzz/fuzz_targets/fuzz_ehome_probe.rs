#![no_main]

use bytes::BytesMut;
use cheetah_codec::EhomeDecoder;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    let mut buf = BytesMut::from(data);
    let mut decoder = EhomeDecoder::new();
    let _ = decoder.decode(&mut buf);
});
