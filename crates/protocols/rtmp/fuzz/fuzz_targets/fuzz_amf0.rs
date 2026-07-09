#![no_main]

use libfuzzer_sys::fuzz_target;
use cheetah_rtmp_core::{AmfValue, AmfVersion};

fuzz_target!(|data: &[u8]| {
    // 尝试解码 AMF0 值
    if let Ok((_size, value)) = AmfValue::decode(data, AmfVersion::Amf0) {
        // 如果解码成功，也尝试重新编码
        let mut encoded = Vec::new();
        value.encode(&mut encoded);
    }
});
