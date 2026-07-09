#![no_main]
use cheetah_codec::CodecId;
use cheetah_hls_core::TsMuxer;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }

    let codec = match data[0] % 5 {
        0 => CodecId::H264,
        1 => CodecId::H265,
        2 => CodecId::VP8,
        3 => CodecId::VP9,
        _ => CodecId::AV1,
    };
    let has_audio = data[1] & 1 == 1;
    let is_keyframe = data[2] & 1 == 1;
    let pts = u64::from(data[3]) * 90000;

    let mut muxer = TsMuxer::new(codec, CodecId::AAC, has_audio);
    muxer.write_pat_pmt();
    muxer.write_video(&data[4..], pts, pts, is_keyframe);

    if has_audio && data.len() > 10 {
        muxer.write_audio(&data[4..10], pts);
    }

    let segment = muxer.take_segment();

    // Invariant: output must be 188-byte aligned
    assert_eq!(segment.len() % 188, 0);

    // Invariant: every packet starts with sync byte
    for chunk in segment.chunks(188) {
        assert_eq!(chunk[0], 0x47);
    }
});
