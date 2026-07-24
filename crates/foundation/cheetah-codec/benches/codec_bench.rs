use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFormat, MediaKind, MpegTsDemuxer, MpegTsDemuxerConfig, MpegTsMuxEvent,
    MpegTsMuxer, MpegTsMuxerConfig, RtpReorderBuffer, RtpReorderSettings, RtpSequenceUnwrapper,
    Timebase, TimestampNormalizeInput, TimestampNormalizeMode, TimestampNormalizer,
    TimestampNormalizerConfig, TimestampValue, TrackId, TrackInfo,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

fn make_h264_video_track_info(pid: u16) -> TrackInfo {
    let mut info = TrackInfo::new(TrackId(pid as u32), MediaKind::Video, CodecId::H264, 90_000);
    info.sample_rate = Some(90_000);
    info
}

fn make_h264_frame(seq: u64, key: bool) -> AVFrame {
    // Minimal Annex-B IDR / non-IDR with start code.
    let payload = if key {
        Bytes::from_static(&[
            0x00, 0x00, 0x00, 0x01, 0x05, 0x00, 0x00, 0x00, 0x00, 0x01, 0x09, 0x10,
        ])
    } else {
        Bytes::from_static(&[
            0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x09, 0x10,
        ])
    };
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

fn mux_one_stream(frames: &[AVFrame]) -> Vec<MpegTsMuxEvent> {
    let config = MpegTsMuxerConfig::default();
    let track_info = make_h264_video_track_info(0x100);
    let mut muxer = MpegTsMuxer::new(&config, &[track_info]);
    let mut all = muxer.write_tables();
    for frame in frames {
        all.extend(muxer.push_frame(frame));
    }
    all
}

fn demux_packets(data: &[u8]) -> Vec<cheetah_codec::MpegTsDemuxEvent> {
    let config = MpegTsDemuxerConfig::default();
    let mut demuxer = MpegTsDemuxer::new(config);
    let mut events = demuxer.push(data);
    events.extend(demuxer.flush());
    events
}

fn normalize_sequence(frames: &[AVFrame]) {
    let config =
        TimestampNormalizerConfig::new(Timebase::new(1, 1_000), Timebase::new(1, 1_000), Some(32))
            .expect("valid config");
    let mut normalizer = TimestampNormalizer::new(config);
    for (i, frame) in frames.iter().enumerate() {
        let _ = normalizer.normalize(TimestampNormalizeInput {
            mode: TimestampNormalizeMode::DtsPts {
                dts: TimestampValue::Unwrapped(frame.dts),
                pts: TimestampValue::Unwrapped(frame.pts),
            },
            frame_duration: if i == 0 { None } else { Some(33) },
            fallback_step: Some(33),
            is_video: true,
            force_discontinuity: false,
        });
    }
}

fn generate_frames(count: usize) -> Vec<AVFrame> {
    (0..count)
        .map(|i| make_h264_frame(i as u64, i % 30 == 0))
        .collect()
}

fn bench_mux_demux(c: &mut Criterion) {
    let mut group = c.benchmark_group("mpeg_ts_mux_demux");
    for size in [100, 1_000, 10_000] {
        let frames = generate_frames(size);
        let events = mux_one_stream(&frames);
        let mut ts_bytes: Vec<u8> = Vec::new();
        for ev in events {
            if let MpegTsMuxEvent::Packet(bytes) = ev {
                ts_bytes.extend_from_slice(&bytes);
            }
        }

        group.bench_with_input(BenchmarkId::new("mux", size), &frames, |b, frames| {
            b.iter(|| black_box(mux_one_stream(black_box(frames))));
        });

        group.bench_with_input(BenchmarkId::new("demux", size), &ts_bytes, |b, data| {
            b.iter(|| black_box(demux_packets(black_box(data))));
        });
    }
    group.finish();
}

fn bench_normalize(c: &mut Criterion) {
    let mut group = c.benchmark_group("timestamp_normalize");
    for size in [100, 1_000, 10_000] {
        let frames = generate_frames(size);
        group.bench_with_input(BenchmarkId::new("pts_only", size), &frames, |b, frames| {
            b.iter(|| {
                normalize_sequence(black_box(frames));
                black_box(())
            });
        });
    }
    group.finish();
}

fn bench_sequence_unwrapper(c: &mut Criterion) {
    let mut group = c.benchmark_group("rtp_sequence_unwrapper");
    for size in [100, 1_000, 10_000] {
        let seqs: Vec<u16> = (0..size).map(|i| i as u16).collect();
        group.bench_with_input(BenchmarkId::new("monotonic", size), &seqs, |b, seqs| {
            b.iter(|| {
                let mut unwrapper = RtpSequenceUnwrapper::new();
                for &raw in black_box(seqs) {
                    black_box(unwrapper.extend(raw));
                }
            });
        });
    }
    group.finish();
}

fn bench_reorder_buffer(c: &mut Criterion) {
    let settings = RtpReorderSettings {
        max_packets: 32,
        max_delay_ms: 100,
    };
    let mut group = c.benchmark_group("rtp_reorder_buffer");
    for size in [100, 1_000, 10_000] {
        let packets: Vec<(u16, u64, u64)> = (0..size)
            .map(|i| (i as u16, i as u64, i as u64 * 30))
            .collect();
        group.bench_with_input(
            BenchmarkId::new("in_order", size),
            &packets,
            |b, packets| {
                b.iter(|| {
                    let mut buf = RtpReorderBuffer::new(settings);
                    for &(seq, payload, arrival) in black_box(packets) {
                        black_box(buf.push(seq, arrival, payload));
                    }
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_mux_demux,
    bench_normalize,
    bench_sequence_unwrapper,
    bench_reorder_buffer
);
criterion_main!(benches);
