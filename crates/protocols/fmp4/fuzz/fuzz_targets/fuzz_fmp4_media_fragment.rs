#![no_main]
use libfuzzer_sys::fuzz_target;

use bytes::Bytes;
use cheetah_codec::{
    CodecId, Fmp4Demuxer, Fmp4DemuxerConfig, Fmp4MuxSample, Fmp4Muxer, Fmp4MuxerConfig,
    MediaKind, TrackId, TrackInfo, track::CodecExtradata,
};

fuzz_target!(|data: &[u8]| {
    // First: fuzz the demuxer with a valid init segment prefix + arbitrary data
    let mut t = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
    t.width = Some(320);
    t.height = Some(240);
    t.extradata = CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1E])],
        pps: vec![Bytes::from_static(&[0x68, 0xCE, 0x38])],
        avcc: Some(Bytes::from_static(&[
            0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E,
            0x01, 0x00, 0x03, 0x68, 0xCE, 0x38,
        ])),
    };
    let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[t]);
    let init = muxer.init_segment();

    let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig { max_box_bytes: 1024 * 1024 });
    if let Some(cheetah_codec::Fmp4MuxEvent::InitSegment(init_data)) = init.into_iter().next() {
        let _ = demuxer.push(&init_data);
    }
    // Feed fuzzed data as media segment
    let _ = demuxer.push(data);
});
