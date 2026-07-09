//! TS codec matrix integration tests: mux/demux roundtrip for all codecs and multi-track combos.

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, MpegTsDemuxDiagnostic, MpegTsDemuxEvent,
    MpegTsDemuxer, MpegTsDemuxerConfig, MpegTsMuxEvent, MpegTsMuxer, MpegTsMuxerConfig, Timebase,
    TrackId, TrackInfo, TS_PACKET_SIZE,
};

fn mux_demux_roundtrip(tracks: &[TrackInfo], frames: &[AVFrame]) -> Vec<MpegTsDemuxEvent> {
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), tracks);
    let mut ts_data = Vec::new();
    for ev in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }
    for frame in frames {
        for ev in muxer.push_frame(frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }
    }
    // Verify 188-byte alignment
    assert_eq!(
        ts_data.len() % TS_PACKET_SIZE,
        0,
        "output must be 188-aligned"
    );

    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&ts_data);
    let flush = demuxer.flush();
    events.into_iter().chain(flush).collect()
}

fn make_video_frame(track_id: u32, codec: CodecId, pts: i64, key: bool) -> AVFrame {
    let payload = match codec {
        CodecId::H264 => Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB]),
        CodecId::H265 => Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xCC]),
        CodecId::H266 => Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x00, 0x38, 0x01, 0xDD]),
        CodecId::MJPEG => Bytes::from_static(&[0xFF, 0xD8, 0xFF, 0xD9]),
        _ => Bytes::from_static(&[0x01, 0x02, 0x03, 0x04]),
    };
    let format = match codec {
        CodecId::H264 | CodecId::H265 | CodecId::H266 => FrameFormat::CanonicalH26x,
        CodecId::AV1 => FrameFormat::CanonicalAv1Obu,
        CodecId::VP8 => FrameFormat::CanonicalVp8Frame,
        CodecId::VP9 => FrameFormat::CanonicalVp9Frame,
        CodecId::MJPEG => FrameFormat::MjpegFrame,
        _ => FrameFormat::Unknown,
    };
    let mut frame = AVFrame::new(
        TrackId(track_id),
        MediaKind::Video,
        codec,
        format,
        pts,
        pts,
        Timebase::new(1, 90_000),
        payload,
    );
    frame.pts_us = pts * 100 / 9;
    frame.dts_us = pts * 100 / 9;
    if key {
        frame.flags.insert(FrameFlags::KEY);
    }
    frame
}

fn make_audio_frame(track_id: u32, codec: CodecId, pts: i64) -> AVFrame {
    let payload = Bytes::from_static(&[0xAA, 0xBB, 0xCC, 0xDD]);
    let format = match codec {
        CodecId::AAC => FrameFormat::AacRaw,
        CodecId::Opus => FrameFormat::OpusPacket,
        CodecId::G711A | CodecId::G711U => FrameFormat::G711Packet,
        CodecId::MP2 => FrameFormat::Mp2Frame,
        CodecId::MP3 => FrameFormat::Mp3Frame,
        CodecId::ADPCM => FrameFormat::AdpcmPacket,
        _ => FrameFormat::Unknown,
    };
    let mut frame = AVFrame::new(
        TrackId(track_id),
        MediaKind::Audio,
        codec,
        format,
        pts,
        pts,
        Timebase::new(1, 90_000),
        payload,
    );
    frame.pts_us = pts * 100 / 9;
    frame.dts_us = pts * 100 / 9;
    frame
}

/// Verify a single codec roundtrips through mux/demux correctly.
fn verify_single_codec(codec: CodecId, kind: MediaKind) {
    let track = TrackInfo::new(TrackId(1), kind, codec, 90_000);
    let frame = if kind == MediaKind::Video {
        make_video_frame(1, codec, 90_000, true)
    } else {
        make_audio_frame(1, codec, 90_000)
    };

    let events = mux_demux_roundtrip(&[track], &[frame]);

    let track_found = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
        .count();
    assert!(track_found >= 1, "{codec:?}: should find track");

    let frames: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let MpegTsDemuxEvent::Frame(f) = e {
                Some(f)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(frames.len(), 1, "{codec:?}: should produce 1 frame");
    assert_eq!(frames[0].codec, codec, "{codec:?}: codec mismatch");
    assert!(frames[0].pts > 0, "{codec:?}: PTS should be positive");
}

#[test]
fn ts_codec_matrix_h264() {
    verify_single_codec(CodecId::H264, MediaKind::Video);
}

#[test]
fn ts_codec_matrix_h265() {
    verify_single_codec(CodecId::H265, MediaKind::Video);
}

#[test]
fn ts_codec_matrix_h266() {
    verify_single_codec(CodecId::H266, MediaKind::Video);
}

#[test]
fn ts_codec_matrix_mjpeg() {
    verify_single_codec(CodecId::MJPEG, MediaKind::Video);
}

#[test]
fn ts_codec_matrix_vp8() {
    verify_single_codec(CodecId::VP8, MediaKind::Video);
}

#[test]
fn ts_codec_matrix_vp9() {
    verify_single_codec(CodecId::VP9, MediaKind::Video);
}

#[test]
fn ts_codec_matrix_av1() {
    verify_single_codec(CodecId::AV1, MediaKind::Video);
}

#[test]
fn ts_codec_matrix_aac() {
    verify_single_codec(CodecId::AAC, MediaKind::Audio);
}

#[test]
fn ts_codec_matrix_g711a() {
    verify_single_codec(CodecId::G711A, MediaKind::Audio);
}

#[test]
fn ts_codec_matrix_g711u() {
    verify_single_codec(CodecId::G711U, MediaKind::Audio);
}

#[test]
fn ts_codec_matrix_opus() {
    verify_single_codec(CodecId::Opus, MediaKind::Audio);
}

#[test]
fn ts_codec_matrix_mp3() {
    verify_single_codec(CodecId::MP3, MediaKind::Audio);
}

#[test]
fn ts_codec_matrix_mp2() {
    verify_single_codec(CodecId::MP2, MediaKind::Audio);
}

#[test]
fn ts_codec_matrix_adpcm() {
    verify_single_codec(CodecId::ADPCM, MediaKind::Audio);
}

#[test]
fn ts_multi_track_h264_aac() {
    let tracks = vec![
        TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
        TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000),
    ];
    let frames = vec![
        make_video_frame(1, CodecId::H264, 90_000, true),
        make_audio_frame(2, CodecId::AAC, 90_000),
    ];
    let events = mux_demux_roundtrip(&tracks, &frames);

    let track_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
        .count();
    assert_eq!(track_count, 2);

    let frame_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
        .count();
    assert_eq!(frame_count, 2);
}

#[test]
fn ts_multi_track_h265_g711a_g711u() {
    let tracks = vec![
        TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H265, 90_000),
        TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::G711A, 8_000),
        TrackInfo::new(TrackId(3), MediaKind::Audio, CodecId::G711U, 8_000),
    ];
    let frames = vec![
        make_video_frame(1, CodecId::H265, 90_000, true),
        make_audio_frame(2, CodecId::G711A, 90_000),
        make_audio_frame(3, CodecId::G711U, 90_000),
    ];
    let events = mux_demux_roundtrip(&tracks, &frames);

    let track_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
        .count();
    assert_eq!(track_count, 3);

    let frame_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
        .count();
    assert_eq!(frame_count, 3);
}

#[test]
fn ts_multi_track_dual_video_dual_audio() {
    let tracks = vec![
        TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
        TrackInfo::new(TrackId(2), MediaKind::Video, CodecId::H265, 90_000),
        TrackInfo::new(TrackId(3), MediaKind::Audio, CodecId::AAC, 48_000),
        TrackInfo::new(TrackId(4), MediaKind::Audio, CodecId::MP3, 90_000),
    ];
    let frames = vec![
        make_video_frame(1, CodecId::H264, 90_000, true),
        make_video_frame(2, CodecId::H265, 90_000, true),
        make_audio_frame(3, CodecId::AAC, 90_000),
        make_audio_frame(4, CodecId::MP3, 90_000),
    ];
    let events = mux_demux_roundtrip(&tracks, &frames);

    let track_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
        .count();
    assert_eq!(track_count, 4);

    let frame_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
        .count();
    assert_eq!(frame_count, 4);
}

#[test]
fn ts_demux_reassembles_multi_packet_pmt() {
    let tracks: Vec<_> = (0..32)
        .map(|idx| TrackInfo::new(TrackId(idx + 1), MediaKind::Audio, CodecId::Opus, 48_000))
        .collect();
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    let mut ts_data = Vec::new();

    for ev in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(data) = ev {
            ts_data.extend_from_slice(&data);
        }
    }

    assert!(
        ts_data.len() > 2 * TS_PACKET_SIZE,
        "descriptor-heavy PMT must span multiple TS packets"
    );

    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&ts_data);
    let track_count = events
        .iter()
        .filter(|event| matches!(event, MpegTsDemuxEvent::TrackFound(_)))
        .count();

    assert_eq!(track_count, tracks.len());
}

#[test]
fn ts_audio_only_aac() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Audio,
        CodecId::AAC,
        48_000,
    )];
    let frames = vec![make_audio_frame(1, CodecId::AAC, 90_000)];
    let events = mux_demux_roundtrip(&tracks, &frames);

    let frame_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
        .count();
    assert_eq!(frame_count, 1);
}

#[test]
fn ts_pts_dts_non_regressive() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    )];
    let frames: Vec<_> = (0..5)
        .map(|i| make_video_frame(1, CodecId::H264, 90_000 * (i + 1), i == 0))
        .collect();
    let events = mux_demux_roundtrip(&tracks, &frames);

    let pts_values: Vec<i64> = events
        .iter()
        .filter_map(|e| {
            if let MpegTsDemuxEvent::Frame(f) = e {
                Some(f.pts)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(pts_values.len(), 5);
    for i in 1..pts_values.len() {
        assert!(
            pts_values[i] > pts_values[i - 1],
            "PTS should be monotonically increasing: {} <= {}",
            pts_values[i],
            pts_values[i - 1]
        );
    }
}

#[test]
fn pure_garbage_does_not_panic() {
    let garbage = vec![0xAA; 10000];
    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&garbage);
    let flush = demuxer.flush();
    // Should not panic; no frames produced from garbage
    let all: Vec<_> = events.into_iter().chain(flush).collect();
    let frame_count = all
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
        .count();
    assert_eq!(frame_count, 0);
}

#[test]
fn truncated_single_byte_does_not_panic() {
    let mut demuxer = MpegTsDemuxer::default();
    // Feed single bytes including sync byte
    for &b in &[0x47, 0x00, 0x11] {
        let _ = demuxer.push(&[b]);
    }
    let _ = demuxer.flush();
}

#[test]
fn sync_byte_corruption_mid_stream() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    )];
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    let mut ts_data = Vec::new();
    for ev in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        90_000,
        90_000,
        Timebase::new(1, 90_000),
        Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xAA]),
    );
    frame.flags = FrameFlags::KEY;
    frame.pts_us = 1_000_000;
    frame.dts_us = 1_000_000;
    for ev in muxer.push_frame(&frame) {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    // Insert garbage between valid packets to force resync
    let mut corrupted = Vec::new();
    corrupted.extend_from_slice(&ts_data[..TS_PACKET_SIZE]); // PAT
    corrupted.extend_from_slice(&[0xBB; 50]); // garbage
    corrupted.extend_from_slice(&ts_data[TS_PACKET_SIZE..]); // rest

    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&corrupted);
    let flush = demuxer.flush();
    let all: Vec<_> = events.into_iter().chain(flush).collect();

    // Should detect sync loss from the garbage insertion
    let has_sync_loss = all.iter().any(|e| {
        matches!(
            e,
            MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::SyncLoss)
        )
    });
    assert!(
        has_sync_loss,
        "should detect sync loss from inserted garbage"
    );
}

#[test]
fn oversized_pes_triggers_overflow_not_panic() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    )];
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    let mut ts_data = Vec::new();
    for ev in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    // Create a large frame that will exceed a small reassembly limit
    let big_payload = vec![0x65; 2000];
    let mut annexb = vec![0x00, 0x00, 0x00, 0x01];
    annexb.extend_from_slice(&big_payload);
    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        90_000,
        90_000,
        Timebase::new(1, 90_000),
        Bytes::from(annexb),
    );
    frame.flags = FrameFlags::KEY;
    frame.pts_us = 1_000_000;
    frame.dts_us = 1_000_000;
    for ev in muxer.push_frame(&frame) {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    // Use tiny reassembly limit
    let config = MpegTsDemuxerConfig {
        max_reassembly_bytes: 200,
        strict_crc: false,
    };
    let mut demuxer = MpegTsDemuxer::new(config);
    let events = demuxer.push(&ts_data);
    let flush = demuxer.flush();
    let all: Vec<_> = events.into_iter().chain(flush).collect();

    let has_overflow = all.iter().any(|e| {
        matches!(
            e,
            MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::PesOverflow { .. })
        )
    });
    assert!(has_overflow, "should detect PES overflow");
}

#[test]
fn empty_input_does_not_panic() {
    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&[]);
    assert!(events.is_empty());
    let flush = demuxer.flush();
    assert!(flush.is_empty());
}

#[test]
fn repeated_pat_pmt_without_pes_does_not_panic() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    )];
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);

    // Write tables multiple times without any frames
    let mut ts_data = Vec::new();
    for _ in 0..10 {
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }
    }

    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&ts_data);
    let flush = demuxer.flush();
    let all: Vec<_> = events.into_iter().chain(flush).collect();

    // Should find track but no frames
    let frame_count = all
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
        .count();
    assert_eq!(frame_count, 0);
}

/// Phase 01.1: Verify demux handles ABL/libmpeg style 0x9C stream_type for Opus.
#[test]
fn ts_opus_0x9c_compat_demux() {
    // Build a custom PAT+PMT with stream_type=0x9C for Opus (ABL/libmpeg style)
    use cheetah_codec::{crc32_mpeg2, encode_timestamp};

    let mut ts_data = Vec::new();

    // PAT packet: program 1 -> PMT PID 0x1000
    let mut pat_pkt = [0xFF_u8; 188];
    pat_pkt[0] = 0x47;
    pat_pkt[1] = 0x40; // PUSI + PID 0x0000
    pat_pkt[2] = 0x00;
    pat_pkt[3] = 0x10; // payload only, CC=0
    pat_pkt[4] = 0x00; // pointer field
    let mut pat_section = Vec::new();
    pat_section.extend_from_slice(&[0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00]);
    pat_section.extend_from_slice(&[0x00, 0x01, 0xF0, 0x00]); // program 1 -> PMT PID 0x1000
    let crc = crc32_mpeg2(&pat_section);
    pat_section.extend_from_slice(&crc.to_be_bytes());
    pat_pkt[5..5 + pat_section.len()].copy_from_slice(&pat_section);
    ts_data.extend_from_slice(&pat_pkt);

    // PMT packet: one audio stream with stream_type=0x9C, PID=0x0110
    let mut pmt_pkt = [0xFF_u8; 188];
    pmt_pkt[0] = 0x47;
    pmt_pkt[1] = 0x50; // PUSI + PID 0x1000
    pmt_pkt[2] = 0x00;
    pmt_pkt[3] = 0x10; // payload only, CC=0
    pmt_pkt[4] = 0x00; // pointer field
    let mut pmt_section = Vec::new();
    pmt_section.extend_from_slice(&[0x02, 0xB0, 0x00]); // table_id, section_syntax + length placeholder
    pmt_section.extend_from_slice(&[0x00, 0x01, 0xC1, 0x00, 0x00]); // program_number, version, section
    pmt_section.extend_from_slice(&[0xE1, 0x10]); // PCR PID = 0x0110
    pmt_section.extend_from_slice(&[0xF0, 0x00]); // program_info_length = 0
                                                  // ES entry: stream_type=0x9C, PID=0x0110, ES_info_length=0
    pmt_section.extend_from_slice(&[0x9C, 0xE1, 0x10, 0xF0, 0x00]);
    // Fix section length
    let section_len = (pmt_section.len() - 3 + 4) as u16;
    let len_bytes = (0xB000 | section_len).to_be_bytes();
    pmt_section[1] = len_bytes[0];
    pmt_section[2] = len_bytes[1];
    let crc = crc32_mpeg2(&pmt_section);
    pmt_section.extend_from_slice(&crc.to_be_bytes());
    pmt_pkt[5..5 + pmt_section.len()].copy_from_slice(&pmt_section);
    ts_data.extend_from_slice(&pmt_pkt);

    // PES packet with Opus payload on PID 0x0110
    let opus_payload = &[0x01, 0x02, 0x03, 0x04];
    let mut pes = Vec::new();
    pes.extend_from_slice(&[0x00, 0x00, 0x01, 0xC0]); // PES start, audio stream_id
    let pes_len = (3 + 5 + opus_payload.len()) as u16;
    pes.extend_from_slice(&pes_len.to_be_bytes());
    pes.push(0x80); // marker bits
    pes.push(0x80); // PTS only
    pes.push(0x05); // header_data_length
    encode_timestamp(&mut pes, 0x02, 90_000);
    pes.extend_from_slice(opus_payload);

    // Wrap PES in TS packet
    let mut pes_pkt = [0xFF_u8; 188];
    pes_pkt[0] = 0x47;
    pes_pkt[1] = 0x41; // PUSI + PID 0x0110
    pes_pkt[2] = 0x10;
    pes_pkt[3] = 0x10; // payload only, CC=0
    pes_pkt[4..4 + pes.len()].copy_from_slice(&pes);
    ts_data.extend_from_slice(&pes_pkt);

    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&ts_data);
    let flush = demuxer.flush();
    let all: Vec<_> = events.into_iter().chain(flush).collect();

    // Should find Opus track
    let track = all.iter().find_map(|e| {
        if let MpegTsDemuxEvent::TrackFound(t) = e {
            Some(t)
        } else {
            None
        }
    });
    assert!(track.is_some(), "should find Opus track from 0x9C");
    assert_eq!(track.unwrap().codec, CodecId::Opus);

    // Should produce a frame
    let frame = all.iter().find_map(|e| {
        if let MpegTsDemuxEvent::Frame(f) = e {
            Some(f)
        } else {
            None
        }
    });
    assert!(frame.is_some(), "should produce Opus frame");
    assert_eq!(frame.unwrap().codec, CodecId::Opus);
}

/// Phase 01.1: Verify 0x06 private stream with unknown descriptor emits UnknownStreamType.
#[test]
fn ts_unknown_private_stream_0x06_diagnostic() {
    use cheetah_codec::crc32_mpeg2;

    let mut ts_data = Vec::new();

    // PAT
    let mut pat_pkt = [0xFF_u8; 188];
    pat_pkt[0] = 0x47;
    pat_pkt[1] = 0x40;
    pat_pkt[2] = 0x00;
    pat_pkt[3] = 0x10;
    pat_pkt[4] = 0x00;
    let mut pat_section = Vec::new();
    pat_section.extend_from_slice(&[0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00]);
    pat_section.extend_from_slice(&[0x00, 0x01, 0xF0, 0x00]);
    let crc = crc32_mpeg2(&pat_section);
    pat_section.extend_from_slice(&crc.to_be_bytes());
    pat_pkt[5..5 + pat_section.len()].copy_from_slice(&pat_section);
    ts_data.extend_from_slice(&pat_pkt);

    // PMT with stream_type=0x06 and unknown registration descriptor "ZZZZ"
    let mut pmt_pkt = [0xFF_u8; 188];
    pmt_pkt[0] = 0x47;
    pmt_pkt[1] = 0x50;
    pmt_pkt[2] = 0x00;
    pmt_pkt[3] = 0x10;
    pmt_pkt[4] = 0x00;
    let mut pmt_section = Vec::new();
    pmt_section.extend_from_slice(&[0x02, 0xB0, 0x00]);
    pmt_section.extend_from_slice(&[0x00, 0x01, 0xC1, 0x00, 0x00]);
    pmt_section.extend_from_slice(&[0xE1, 0x10]); // PCR PID
    pmt_section.extend_from_slice(&[0xF0, 0x00]); // program_info_length = 0
                                                  // ES entry: stream_type=0x06, PID=0x0110, ES_info with unknown descriptor
    let es_info = &[0x05, 0x04, b'Z', b'Z', b'Z', b'Z']; // registration "ZZZZ"
    pmt_section.push(0x06); // stream_type
    pmt_section.extend_from_slice(&[0xE1, 0x10]); // PID 0x0110
    let es_info_len = es_info.len() as u16;
    pmt_section.extend_from_slice(&(0xF000 | es_info_len).to_be_bytes());
    pmt_section.extend_from_slice(es_info);
    // Fix section length
    let section_len = (pmt_section.len() - 3 + 4) as u16;
    let len_bytes = (0xB000 | section_len).to_be_bytes();
    pmt_section[1] = len_bytes[0];
    pmt_section[2] = len_bytes[1];
    let crc = crc32_mpeg2(&pmt_section);
    pmt_section.extend_from_slice(&crc.to_be_bytes());
    pmt_pkt[5..5 + pmt_section.len()].copy_from_slice(&pmt_section);
    ts_data.extend_from_slice(&pmt_pkt);

    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&ts_data);

    let has_unknown = events.iter().any(|e| {
        matches!(
            e,
            MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::UnknownStreamType {
                stream_type: 0x06,
                pid: 0x0110,
            })
        )
    });
    assert!(
        has_unknown,
        "should emit UnknownStreamType for 0x06 with unrecognized descriptor"
    );
}

/// Phase 01.1: Verify VP8/VP9/AV1 registration descriptor roundtrip through mux/demux.
#[test]
fn ts_vp8_vp9_av1_registration_descriptor_roundtrip() {
    for (codec, kind) in [
        (CodecId::VP8, MediaKind::Video),
        (CodecId::VP9, MediaKind::Video),
        (CodecId::AV1, MediaKind::Video),
    ] {
        let track = TrackInfo::new(TrackId(1), kind, codec, 90_000);
        let frame = make_video_frame(1, codec, 90_000, true);
        let events = mux_demux_roundtrip(&[track], &[frame]);

        let found_track = events.iter().find_map(|e| {
            if let MpegTsDemuxEvent::TrackFound(t) = e {
                Some(t)
            } else {
                None
            }
        });
        assert!(
            found_track.is_some(),
            "{codec:?}: should find track via registration descriptor"
        );
        assert_eq!(found_track.unwrap().codec, codec);
    }
}

/// Phase 01.2: Verify demux splits multiple ADTS frames from a single PES.
#[test]
fn ts_aac_adts_multi_frame_split() {
    use cheetah_codec::{crc32_mpeg2, encode_timestamp, AdtsHeader};

    let mut ts_data = Vec::new();

    // PAT
    let mut pat_pkt = [0xFF_u8; 188];
    pat_pkt[0] = 0x47;
    pat_pkt[1] = 0x40;
    pat_pkt[2] = 0x00;
    pat_pkt[3] = 0x10;
    pat_pkt[4] = 0x00;
    let mut pat_section = Vec::new();
    pat_section.extend_from_slice(&[0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00]);
    pat_section.extend_from_slice(&[0x00, 0x01, 0xF0, 0x00]);
    let crc = crc32_mpeg2(&pat_section);
    pat_section.extend_from_slice(&crc.to_be_bytes());
    pat_pkt[5..5 + pat_section.len()].copy_from_slice(&pat_section);
    ts_data.extend_from_slice(&pat_pkt);

    // PMT with AAC stream_type=0x0F, PID=0x0110
    let mut pmt_pkt = [0xFF_u8; 188];
    pmt_pkt[0] = 0x47;
    pmt_pkt[1] = 0x50;
    pmt_pkt[2] = 0x00;
    pmt_pkt[3] = 0x10;
    pmt_pkt[4] = 0x00;
    let mut pmt_section = Vec::new();
    pmt_section.extend_from_slice(&[0x02, 0xB0, 0x00]);
    pmt_section.extend_from_slice(&[0x00, 0x01, 0xC1, 0x00, 0x00]);
    pmt_section.extend_from_slice(&[0xE1, 0x10]);
    pmt_section.extend_from_slice(&[0xF0, 0x00]);
    pmt_section.extend_from_slice(&[0x0F, 0xE1, 0x10, 0xF0, 0x00]); // AAC
    let section_len = (pmt_section.len() - 3 + 4) as u16;
    let len_bytes = (0xB000 | section_len).to_be_bytes();
    pmt_section[1] = len_bytes[0];
    pmt_section[2] = len_bytes[1];
    let crc = crc32_mpeg2(&pmt_section);
    pmt_section.extend_from_slice(&crc.to_be_bytes());
    pmt_pkt[5..5 + pmt_section.len()].copy_from_slice(&pmt_section);
    ts_data.extend_from_slice(&pmt_pkt);

    // Build PES with 3 consecutive ADTS frames
    // AAC-LC, 44100Hz (index=4), stereo (ch=2)
    let aac_raw_1 = vec![0xAA; 10];
    let aac_raw_2 = vec![0xBB; 12];
    let aac_raw_3 = vec![0xCC; 8];

    let make_adts = |raw: &[u8]| -> Vec<u8> {
        let h = AdtsHeader {
            profile: 1,                  // AAC-LC
            sampling_frequency_index: 4, // 44100
            channel_configuration: 2,
            frame_length: (raw.len() + 7) as u16,
        };
        let header = h.build();
        let mut out = Vec::new();
        out.extend_from_slice(&header);
        out.extend_from_slice(raw);
        out
    };

    let mut adts_payload = Vec::new();
    adts_payload.extend_from_slice(&make_adts(&aac_raw_1));
    adts_payload.extend_from_slice(&make_adts(&aac_raw_2));
    adts_payload.extend_from_slice(&make_adts(&aac_raw_3));

    // Build PES
    let mut pes = Vec::new();
    pes.extend_from_slice(&[0x00, 0x00, 0x01, 0xC0]); // audio stream_id
    let pes_len = (3 + 5 + adts_payload.len()) as u16;
    pes.extend_from_slice(&pes_len.to_be_bytes());
    pes.push(0x80);
    pes.push(0x80); // PTS only
    pes.push(0x05);
    encode_timestamp(&mut pes, 0x02, 90_000);
    pes.extend_from_slice(&adts_payload);

    // Wrap PES in TS packets (may need multiple)
    let mut pes_offset = 0;
    let mut cc: u8 = 0;
    let mut first = true;
    while pes_offset < pes.len() {
        let mut pkt = [0xFF_u8; 188];
        pkt[0] = 0x47;
        if first {
            pkt[1] = 0x41; // PUSI + PID 0x0110
        } else {
            pkt[1] = 0x01; // PID 0x0110
        }
        pkt[2] = 0x10;
        pkt[3] = 0x10 | (cc & 0x0F);
        let available = 188 - 4;
        let copy_len = (pes.len() - pes_offset).min(available);
        pkt[4..4 + copy_len].copy_from_slice(&pes[pes_offset..pes_offset + copy_len]);
        pes_offset += copy_len;
        ts_data.extend_from_slice(&pkt);
        cc = (cc + 1) & 0x0F;
        first = false;
    }

    // Add a second PES to trigger flush of the first (demux flushes on next PUSI)
    let mut pes2 = Vec::new();
    pes2.extend_from_slice(&[0x00, 0x00, 0x01, 0xC0]);
    let adts2 = make_adts(&[0xDD; 5]);
    let pes2_len = (3 + 5 + adts2.len()) as u16;
    pes2.extend_from_slice(&pes2_len.to_be_bytes());
    pes2.push(0x80);
    pes2.push(0x80);
    pes2.push(0x05);
    encode_timestamp(&mut pes2, 0x02, 180_000);
    pes2.extend_from_slice(&adts2);

    let mut pkt2 = [0xFF_u8; 188];
    pkt2[0] = 0x47;
    pkt2[1] = 0x41; // PUSI
    pkt2[2] = 0x10;
    pkt2[3] = 0x10 | (cc & 0x0F);
    let copy_len = pes2.len().min(184);
    pkt2[4..4 + copy_len].copy_from_slice(&pes2[..copy_len]);
    ts_data.extend_from_slice(&pkt2);

    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&ts_data);
    let flush = demuxer.flush();
    let all: Vec<_> = events.into_iter().chain(flush).collect();

    let frames: Vec<_> = all
        .iter()
        .filter_map(|e| {
            if let MpegTsDemuxEvent::Frame(f) = e {
                Some(f)
            } else {
                None
            }
        })
        .collect();

    // Should produce 3 frames from first PES + 1 from second PES = 4 total
    assert_eq!(
        frames.len(),
        4,
        "should split 3 ADTS frames from first PES + 1 from second"
    );

    // Verify payloads are stripped of ADTS headers (raw AAC)
    assert_eq!(&frames[0].payload[..], &aac_raw_1[..]);
    assert_eq!(&frames[1].payload[..], &aac_raw_2[..]);
    assert_eq!(&frames[2].payload[..], &aac_raw_3[..]);

    // Verify PTS increments: 1024 samples at 44100Hz in 90kHz ticks = 1024*90000/44100 ≈ 2089
    assert!(frames[1].pts > frames[0].pts);
    assert!(frames[2].pts > frames[1].pts);
}

/// Phase 01.2: Verify ADTS length validation diagnostic.
#[test]
fn ts_aac_adts_invalid_length_diagnostic() {
    use cheetah_codec::{crc32_mpeg2, encode_timestamp};

    let mut ts_data = Vec::new();

    // PAT
    let mut pat_pkt = [0xFF_u8; 188];
    pat_pkt[0] = 0x47;
    pat_pkt[1] = 0x40;
    pat_pkt[2] = 0x00;
    pat_pkt[3] = 0x10;
    pat_pkt[4] = 0x00;
    let mut pat_section = Vec::new();
    pat_section.extend_from_slice(&[0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00]);
    pat_section.extend_from_slice(&[0x00, 0x01, 0xF0, 0x00]);
    let crc = crc32_mpeg2(&pat_section);
    pat_section.extend_from_slice(&crc.to_be_bytes());
    pat_pkt[5..5 + pat_section.len()].copy_from_slice(&pat_section);
    ts_data.extend_from_slice(&pat_pkt);

    // PMT with AAC
    let mut pmt_pkt = [0xFF_u8; 188];
    pmt_pkt[0] = 0x47;
    pmt_pkt[1] = 0x50;
    pmt_pkt[2] = 0x00;
    pmt_pkt[3] = 0x10;
    pmt_pkt[4] = 0x00;
    let mut pmt_section = Vec::new();
    pmt_section.extend_from_slice(&[0x02, 0xB0, 0x00]);
    pmt_section.extend_from_slice(&[0x00, 0x01, 0xC1, 0x00, 0x00]);
    pmt_section.extend_from_slice(&[0xE1, 0x10]);
    pmt_section.extend_from_slice(&[0xF0, 0x00]);
    pmt_section.extend_from_slice(&[0x0F, 0xE1, 0x10, 0xF0, 0x00]);
    let section_len = (pmt_section.len() - 3 + 4) as u16;
    let len_bytes = (0xB000 | section_len).to_be_bytes();
    pmt_section[1] = len_bytes[0];
    pmt_section[2] = len_bytes[1];
    let crc = crc32_mpeg2(&pmt_section);
    pmt_section.extend_from_slice(&crc.to_be_bytes());
    pmt_pkt[5..5 + pmt_section.len()].copy_from_slice(&pmt_section);
    ts_data.extend_from_slice(&pmt_pkt);

    // Build PES with ADTS frame that claims length > actual payload
    let mut bad_adts = [0u8; 7];
    bad_adts[0] = 0xFF;
    bad_adts[1] = 0xF1;
    bad_adts[2] = 0x50; // AAC-LC, 44100Hz
                        // Set frame_length to 500 (way more than available)
    let frame_len: u16 = 500;
    bad_adts[3] = 0x40 | ((frame_len >> 11) as u8 & 0x03);
    bad_adts[4] = (frame_len >> 3) as u8;
    bad_adts[5] = ((frame_len & 0x07) as u8) << 5 | 0x1F;
    bad_adts[6] = 0xFC;

    let mut pes = Vec::new();
    pes.extend_from_slice(&[0x00, 0x00, 0x01, 0xC0]);
    let pes_len = (3 + 5 + bad_adts.len()) as u16;
    pes.extend_from_slice(&pes_len.to_be_bytes());
    pes.push(0x80);
    pes.push(0x80);
    pes.push(0x05);
    encode_timestamp(&mut pes, 0x02, 90_000);
    pes.extend_from_slice(&bad_adts);

    let mut pkt = [0xFF_u8; 188];
    pkt[0] = 0x47;
    pkt[1] = 0x41;
    pkt[2] = 0x10;
    pkt[3] = 0x10;
    pkt[4..4 + pes.len()].copy_from_slice(&pes);
    ts_data.extend_from_slice(&pkt);

    // Trigger flush with another PUSI
    let mut pkt2 = [0xFF_u8; 188];
    pkt2[0] = 0x47;
    pkt2[1] = 0x41;
    pkt2[2] = 0x10;
    pkt2[3] = 0x11;
    pkt2[4] = 0x00;
    pkt2[5] = 0x00;
    pkt2[6] = 0x01;
    pkt2[7] = 0xC0;
    ts_data.extend_from_slice(&pkt2);

    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&ts_data);
    let flush = demuxer.flush();
    let all: Vec<_> = events.into_iter().chain(flush).collect();

    let has_adts_error = all.iter().any(|e| {
        matches!(
            e,
            MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::AdtsError { .. })
        )
    });
    assert!(
        has_adts_error,
        "should emit AdtsError for invalid ADTS length"
    );
}

/// Phase 01.2: Verify muxer does not double-wrap already-ADTS payload.
#[test]
fn ts_aac_mux_no_double_wrap() {
    use cheetah_codec::AdtsHeader;

    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Audio,
        CodecId::AAC,
        48_000,
    )];
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    let mut ts_data = Vec::new();
    for ev in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    // Create a frame that already has ADTS header
    let adts_frame = AdtsHeader {
        profile: 1,
        sampling_frequency_index: 3, // 48000
        channel_configuration: 2,
        frame_length: 7 + 10,
    }
    .build();
    let mut payload_with_adts = Vec::from(&adts_frame[..]);
    payload_with_adts.extend_from_slice(&[0xAA; 10]);

    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Audio,
        CodecId::AAC,
        FrameFormat::AacRaw, // even though it has ADTS, muxer should detect
        90_000,
        90_000,
        Timebase::new(1, 90_000),
        Bytes::from(payload_with_adts.clone()),
    );
    frame.pts_us = 1_000_000;
    frame.dts_us = 1_000_000;
    for ev in muxer.push_frame(&frame) {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    // Demux and verify we get back the original raw AAC (not double-wrapped)
    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&ts_data);
    let flush = demuxer.flush();
    let all: Vec<_> = events.into_iter().chain(flush).collect();

    let frames: Vec<_> = all
        .iter()
        .filter_map(|e| {
            if let MpegTsDemuxEvent::Frame(f) = e {
                Some(f)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(frames.len(), 1);
    // The demuxer strips ADTS, so we should get back the raw 10 bytes
    assert_eq!(&frames[0].payload[..], &[0xAA; 10]);
}

/// Phase 01.3: Verify G711 duration is derived from payload length.
#[test]
fn ts_g711_duration_from_payload_length() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Audio,
        CodecId::G711A,
        8_000,
    )];
    // 160 bytes = 160 samples at 8000Hz = 20ms
    let payload = Bytes::from(vec![0x55u8; 160]);
    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Audio,
        CodecId::G711A,
        FrameFormat::G711Packet,
        90_000,
        90_000,
        Timebase::new(1, 90_000),
        payload,
    );
    frame.pts_us = 1_000_000;
    frame.dts_us = 1_000_000;

    let events = mux_demux_roundtrip(&tracks, &[frame]);

    let demuxed = events.iter().find_map(|e| {
        if let MpegTsDemuxEvent::Frame(f) = e {
            Some(f)
        } else {
            None
        }
    });
    assert!(demuxed.is_some());
    let f = demuxed.unwrap();
    // 160 samples at 8000Hz = 20000us
    assert_eq!(f.duration_us, 20_000);
    // In 90kHz ticks: 160 * 90000 / 8000 = 1800
    assert_eq!(f.duration, 1800);
}

/// Phase 01.3: Verify g711_duration_us helper directly.
#[test]
fn ts_g711_duration_helper() {
    use cheetah_codec::{g711_duration_90k, g711_duration_us};

    // 320 bytes at 8000Hz = 40ms = 40000us
    assert_eq!(g711_duration_us(320, 8000), 40_000);
    // 320 bytes at 8000Hz in 90kHz ticks = 320 * 90000 / 8000 = 3600
    assert_eq!(g711_duration_90k(320, 8000), 3600);
    // Edge case: 0 bytes
    assert_eq!(g711_duration_us(0, 8000), 0);
    // Edge case: 0 sample rate
    assert_eq!(g711_duration_us(160, 0), 0);
}

/// Phase 01.4: Verify FrameRateEstimator warmup and fps clamp.
#[test]
fn ts_frame_rate_estimator_warmup_and_clamp() {
    use cheetah_codec::FrameRateEstimator;

    // ABL-style: 15 warmup, max 120fps, window of 20 samples
    let mut est = FrameRateEstimator::with_abl_defaults(20);

    // Feed 16 warmup frames with wildly varying intervals (should be ignored)
    // The 16th frame establishes last_pts_us for the first real delta
    for i in 0..16 {
        let pts = i * 100_000; // 100ms intervals - unstable startup
        assert_eq!(est.on_frame(pts as i64), None);
    }

    // Feed 20 more frames at 30fps (33333us intervals) starting from last warmup PTS
    let base_pts = 15 * 100_000i64;
    for i in 1..=20 {
        let pts = base_pts + i * 33_333;
        let _ = est.on_frame(pts);
    }

    let fps = est.estimated_fps().unwrap();
    assert!(
        (fps - 30.0).abs() < 1.5,
        "should estimate ~30fps, got {fps}"
    );

    // Test max clamp: feed frames at 200fps (5000us intervals)
    let mut est2 = FrameRateEstimator::with_abl_defaults(10);
    for i in 0..30 {
        est2.on_frame(i * 5_000);
    }
    if let Some(fps) = est2.estimated_fps() {
        assert!(fps <= 120.0, "should clamp to max 120fps, got {fps}");
    }
}

/// Phase 01.5: Verify strict CRC mode rejects PAT/PMT with bad CRC.
#[test]
fn ts_strict_crc_rejects_bad_pat() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    )];
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    let mut ts_data = Vec::new();
    for ev in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    // Corrupt PAT CRC: PAT section starts at byte 5 (after sync+pid+cc+pointer)
    // Section: table_id(1) + length(2) + tsid(2) + ver(1) + secnum(1) + lastsec(1) + prog(2) + pmt_pid(2) + CRC(4) = 16 bytes
    // CRC is at offset 5 + 12 = 17..20
    ts_data[17] ^= 0xFF;

    // Strict mode: should reject and not find PMT PID
    let config = MpegTsDemuxerConfig {
        max_reassembly_bytes: 4 * 1024 * 1024,
        strict_crc: true,
    };
    let mut demuxer = MpegTsDemuxer::new(config);
    let events = demuxer.push(&ts_data);

    let has_crc_error = events.iter().any(|e| {
        matches!(
            e,
            MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::CrcError { pid: 0x0000 })
        )
    });
    assert!(has_crc_error, "should report CRC error on PAT");

    // In strict mode, no tracks should be found (PAT rejected -> no PMT PID)
    let track_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
        .count();
    assert_eq!(track_count, 0, "strict CRC should reject bad PAT");
}

/// Phase 01.5: Verify loose CRC mode continues parsing despite bad CRC.
#[test]
fn ts_loose_crc_continues_parsing() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    )];
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    let mut ts_data = Vec::new();
    for ev in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    // Corrupt PAT CRC
    ts_data[17] ^= 0xFF;

    // Loose mode (default): should continue and find tracks
    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&ts_data);

    let has_crc_error = events.iter().any(|e| {
        matches!(
            e,
            MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::CrcError { .. })
        )
    });
    assert!(has_crc_error, "should report CRC error");

    // But should still find tracks (loose mode continues)
    let track_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
        .count();
    assert!(track_count >= 1, "loose CRC should continue parsing");
}

/// Phase 01.5: Verify continuity gap + PUSI resync produces valid frames.
#[test]
fn ts_continuity_gap_pusi_resync() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    )];
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    let mut ts_data = Vec::new();
    for ev in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    // Write 3 frames
    for i in 0..3 {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            90_000 * (i + 1),
            90_000 * (i + 1),
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xEE]),
        );
        frame.flags = FrameFlags::KEY;
        frame.pts_us = (i + 1) * 1_000_000;
        frame.dts_us = (i + 1) * 1_000_000;
        for ev in muxer.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }
    }

    // Corrupt CC of the second video frame's packet to create a gap
    let pkt_count = ts_data.len() / 188;
    let mut video_pkt_indices = Vec::new();
    for i in 0..pkt_count {
        let off = i * 188;
        let pid = ((ts_data[off + 1] as u16 & 0x1F) << 8) | ts_data[off + 2] as u16;
        let pusi = ts_data[off + 1] & 0x40 != 0;
        if pid == 0x0100 && pusi {
            video_pkt_indices.push(i);
        }
    }
    // Corrupt CC of second video PUSI packet
    if video_pkt_indices.len() >= 2 {
        let off = video_pkt_indices[1] * 188;
        ts_data[off + 3] = (ts_data[off + 3] & 0xF0) | ((ts_data[off + 3] + 7) & 0x0F);
    }

    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&ts_data);
    let flush = demuxer.flush();
    let all: Vec<_> = events.into_iter().chain(flush).collect();

    // Should have continuity gap diagnostic
    let has_gap = all.iter().any(|e| {
        matches!(
            e,
            MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::ContinuityGap { .. })
        )
    });
    assert!(has_gap, "should detect continuity gap");

    // Should still produce frames (PUSI resync)
    let frame_count = all
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
        .count();
    assert!(frame_count >= 2, "should produce frames despite CC gap");
}

/// Phase 01.5: Verify per-PID PES overflow limit.
#[test]
fn ts_per_pid_pes_overflow_limit() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    )];
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    let mut ts_data = Vec::new();
    for ev in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    // Create a large frame
    let big = vec![0x65; 5000];
    let mut annexb = vec![0x00, 0x00, 0x00, 0x01];
    annexb.extend_from_slice(&big);
    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        90_000,
        90_000,
        Timebase::new(1, 90_000),
        Bytes::from(annexb),
    );
    frame.flags = FrameFlags::KEY;
    frame.pts_us = 1_000_000;
    frame.dts_us = 1_000_000;
    for ev in muxer.push_frame(&frame) {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    // Use very small reassembly limit
    let config = MpegTsDemuxerConfig {
        max_reassembly_bytes: 500,
        strict_crc: false,
    };
    let mut demuxer = MpegTsDemuxer::new(config);
    let events = demuxer.push(&ts_data);
    let flush = demuxer.flush();
    let all: Vec<_> = events.into_iter().chain(flush).collect();

    let has_overflow = all.iter().any(|e| {
        matches!(
            e,
            MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::PesOverflow { pid: 0x0100 })
        )
    });
    assert!(has_overflow, "should detect PES overflow on video PID");
}

/// Phase 04.1: Verify PID assignment is stable and sorted by TrackId.
#[test]
fn ts_multi_track_pid_stability_sorted_by_track_id() {
    // Provide tracks in non-sorted order
    let tracks = vec![
        TrackInfo::new(TrackId(5), MediaKind::Audio, CodecId::AAC, 48_000),
        TrackInfo::new(TrackId(3), MediaKind::Video, CodecId::H265, 90_000),
        TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
        TrackInfo::new(TrackId(4), MediaKind::Audio, CodecId::G711A, 8_000),
    ];

    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    let mut ts_data = Vec::new();
    for ev in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data.extend_from_slice(&d);
        }
    }

    // Demux and check track discovery order
    let mut demuxer = MpegTsDemuxer::default();
    let events = demuxer.push(&ts_data);

    let found_tracks: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let MpegTsDemuxEvent::TrackFound(t) = e {
                Some(t)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(found_tracks.len(), 4);

    // Video tracks should get PIDs 0x0100, 0x0101 (sorted by TrackId: 1, 3)
    // Audio tracks should get PIDs 0x0110, 0x0111 (sorted by TrackId: 4, 5)
    // The demuxer assigns new TrackIds, but we can verify the PID pattern
    // by checking that video codecs come first in PMT

    // Verify all 4 tracks are found
    let video_count = found_tracks
        .iter()
        .filter(|t| t.media_kind == MediaKind::Video)
        .count();
    let audio_count = found_tracks
        .iter()
        .filter(|t| t.media_kind == MediaKind::Audio)
        .count();
    assert_eq!(video_count, 2);
    assert_eq!(audio_count, 2);

    // Verify that regardless of input order, the same tracks produce the same output
    // by creating a second muxer with different input order
    let tracks2 = vec![
        TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
        TrackInfo::new(TrackId(4), MediaKind::Audio, CodecId::G711A, 8_000),
        TrackInfo::new(TrackId(3), MediaKind::Video, CodecId::H265, 90_000),
        TrackInfo::new(TrackId(5), MediaKind::Audio, CodecId::AAC, 48_000),
    ];
    let mut muxer2 = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks2);
    let mut ts_data2 = Vec::new();
    for ev in muxer2.write_tables() {
        if let MpegTsMuxEvent::Packet(d) = ev {
            ts_data2.extend_from_slice(&d);
        }
    }

    // Both should produce identical PAT/PMT
    assert_eq!(
        ts_data, ts_data2,
        "PID assignment should be stable regardless of input order"
    );
}

/// Phase 04.2: H264 + AAC + OPUS multi-track.
#[test]
fn ts_multi_track_h264_aac_opus() {
    let tracks = vec![
        TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
        TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000),
        TrackInfo::new(TrackId(3), MediaKind::Audio, CodecId::Opus, 48_000),
    ];
    let frames = vec![
        make_video_frame(1, CodecId::H264, 90_000, true),
        make_audio_frame(2, CodecId::AAC, 90_000),
        make_audio_frame(3, CodecId::Opus, 90_000),
    ];
    let events = mux_demux_roundtrip(&tracks, &frames);

    let track_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
        .count();
    assert_eq!(track_count, 3);

    let frame_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
        .count();
    assert_eq!(frame_count, 3);
}

/// Phase 04.2: Audio-only G711A.
#[test]
fn ts_audio_only_g711a() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Audio,
        CodecId::G711A,
        8_000,
    )];
    let frames = vec![make_audio_frame(1, CodecId::G711A, 90_000)];
    let events = mux_demux_roundtrip(&tracks, &frames);

    let frame_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
        .count();
    assert_eq!(frame_count, 1);
}

/// Phase 04.2: Audio-only G711U.
#[test]
fn ts_audio_only_g711u() {
    let tracks = vec![TrackInfo::new(
        TrackId(1),
        MediaKind::Audio,
        CodecId::G711U,
        8_000,
    )];
    let frames = vec![make_audio_frame(1, CodecId::G711U, 90_000)];
    let events = mux_demux_roundtrip(&tracks, &frames);

    let frame_count = events
        .iter()
        .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
        .count();
    assert_eq!(frame_count, 1);
}

/// Phase 04.3: RTP header extension overflow does not panic.
#[test]
fn ts_rtp_header_extension_overflow_no_panic() {
    use cheetah_codec::RtpPacket;

    // Build RTP with extension that claims more data than available
    let mut pkt = Vec::new();
    // V=2, P=0, X=1, CC=0
    pkt.push(0x90);
    pkt.push(33); // PT=33
    pkt.extend_from_slice(&1u16.to_be_bytes());
    pkt.extend_from_slice(&0u32.to_be_bytes());
    pkt.extend_from_slice(&1u32.to_be_bytes());
    // Extension header: profile=0x0000, length=999 words (way more than available)
    pkt.extend_from_slice(&[0x00, 0x00, 0x03, 0xE7]);
    // Only 4 bytes of actual data
    pkt.extend_from_slice(&[0x47, 0x00, 0x00, 0x00]);

    // Should return None (parse failure), not panic
    let result = RtpPacket::parse(&pkt);
    assert!(result.is_none(), "should fail to parse oversized extension");
}

/// Phase 04.3: Verify all fault scenarios don't panic with RTP-TS ingest.
#[test]
fn ts_rtp_ingest_fault_scenarios_no_panic() {
    use cheetah_codec::RtpPacket;

    // Empty packet
    assert!(RtpPacket::parse(&[]).is_none());

    // Single byte
    assert!(RtpPacket::parse(&[0x80]).is_none());

    // Version 0
    assert!(RtpPacket::parse(&[0x00; 20]).is_some()); // parses but version=0

    // Valid header but empty payload
    let pkt = RtpPacket::parse(&[
        0x80, 33, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xE7,
    ]);
    assert!(pkt.is_some());
    assert!(pkt.unwrap().payload.is_empty());

    // All of the above should not panic
}
