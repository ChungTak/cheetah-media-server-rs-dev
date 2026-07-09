//! FLV Audio/Video Frame Fuzzing
//!
//! 对 FLV 格式的音频/视频帧解码进行模糊测试。

#![no_main]

use libfuzzer_sys::fuzz_target;
use cheetah_rtmp_core::{decode_audio_frame, decode_video_frame, RtmpTimestamp};

fuzz_target!(|data: &[u8]| {
    let timestamp = RtmpTimestamp::ZERO;
    
    // 尝试解码音频帧
    if let Ok(frame) = decode_audio_frame(data, timestamp) {
        // 如果解码成功，也尝试重新编码
        let mut encoded = Vec::new();
        cheetah_rtmp_core::encode_audio_frame(&mut encoded, &frame);
    }
    
    // 尝试解码视频帧
    if let Ok(frame) = decode_video_frame(data, timestamp) {
        // 如果解码成功，也尝试重新编码
        let mut encoded = Vec::new();
        cheetah_rtmp_core::encode_video_frame(&mut encoded, &frame);
    }
});
