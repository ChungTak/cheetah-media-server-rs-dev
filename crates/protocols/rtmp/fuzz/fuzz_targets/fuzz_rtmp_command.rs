//! RTMP Command Fuzzing
//!
//! 对 RTMP 命令解析进行模糊测试。

#![no_main]

use libfuzzer_sys::fuzz_target;
use cheetah_rtmp_core::{AmfValue, AmfVersion, RtmpCommand, TransactionId};

fuzz_target!(|data: &[u8]| {
    // 先尝试按 AMF0 值解码
    if let Ok((_size, object)) = AmfValue::decode(data, AmfVersion::Amf0) {
        // 尝试常见的命令名
        let command_names = ["connect", "createStream", "publish", "play", "deleteStream", "_result", "onStatus"];

        for name in command_names {
            let transaction_id = TransactionId::from_f64(1.0);
            let _ = RtmpCommand::from_message(name, transaction_id, object.clone(), vec![]);
        }
    }
});
