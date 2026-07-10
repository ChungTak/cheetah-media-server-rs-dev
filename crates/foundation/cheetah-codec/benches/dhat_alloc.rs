#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFormat, MediaKind, MpegTsMuxer, MpegTsMuxerConfig, Timebase, TrackId,
    TrackInfo,
};

fn make_h264_video_track_info() -> TrackInfo {
    let mut info = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
    info.sample_rate = Some(90_000);
    info
}

fn make_h264_frame(seq: u64) -> AVFrame {
    let payload = Bytes::from_static(&[
        0x00, 0x00, 0x00, 0x01, 0x05, 0x00, 0x00, 0x00, 0x00, 0x01, 0x09, 0x10,
    ]);
    AVFrame::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        seq as i64,
        seq as i64,
        Timebase::new(1, 1_000),
        payload,
    )
}

fn main() {
    let _profiler = dhat::Profiler::new_heap();

    let config = MpegTsMuxerConfig::default();
    let track_info = make_h264_video_track_info();
    let mut muxer = MpegTsMuxer::new(&config, &[track_info]);

    // Run enough frames to warm up allocations.
    for i in 0..1_000 {
        let frame = make_h264_frame(i);
        let _events = muxer.push_frame(&frame);
    }

    // Write tables one more time.
    let _tables = muxer.write_tables();
}
