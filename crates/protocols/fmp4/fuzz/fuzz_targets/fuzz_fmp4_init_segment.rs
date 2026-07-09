#![no_main]
use libfuzzer_sys::fuzz_target;

use bytes::Bytes;
use cheetah_codec::{
    CodecId, Fmp4Demuxer, Fmp4DemuxerConfig, Fmp4MuxEvent, Fmp4Muxer, Fmp4MuxerConfig, MediaKind,
    TrackId, TrackInfo, track::CodecExtradata,
};

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }

    // Use first byte to select codec, rest as extradata
    let codec_selector = data[0] % 12;
    let extradata = Bytes::copy_from_slice(&data[1..]);

    let (codec, media_kind) = match codec_selector {
        0 => (CodecId::H264, MediaKind::Video),
        1 => (CodecId::H265, MediaKind::Video),
        2 => (CodecId::AAC, MediaKind::Audio),
        3 => (CodecId::G711A, MediaKind::Audio),
        4 => (CodecId::G711U, MediaKind::Audio),
        5 => (CodecId::Opus, MediaKind::Audio),
        6 => (CodecId::MJPEG, MediaKind::Video),
        7 => (CodecId::MP3, MediaKind::Audio),
        8 => (CodecId::MP2, MediaKind::Audio),
        9 => (CodecId::VP8, MediaKind::Video),
        10 => (CodecId::VP9, MediaKind::Video),
        _ => (CodecId::AV1, MediaKind::Video),
    };

    let mut t = TrackInfo::new(TrackId(1), media_kind, codec, 90_000);
    if media_kind == MediaKind::Video {
        t.width = Some(320);
        t.height = Some(240);
    } else {
        t.sample_rate = Some(44_100);
        t.channels = Some(2);
    }
    t.extradata = match codec {
        CodecId::H264 => CodecExtradata::H264 {
            sps: vec![],
            pps: vec![],
            avcc: Some(extradata),
        },
        CodecId::H265 => CodecExtradata::H265 {
            vps: vec![],
            sps: vec![],
            pps: vec![],
            hvcc: Some(extradata),
        },
        CodecId::AAC => CodecExtradata::AAC { asc: extradata },
        _ => CodecExtradata::None,
    };

    let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[t]);
    let events = muxer.init_segment();

    // Feed the generated init segment back into the demuxer
    if let Some(Fmp4MuxEvent::InitSegment(init_data)) = events.into_iter().next() {
        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig { max_box_bytes: 1024 * 1024 });
        let _ = demuxer.push(&init_data);
    }
});
