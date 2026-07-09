use bytes::Bytes;
use cheetah_codec::{
    track::CodecExtradata, CodecId, Fmp4DemuxEvent, Fmp4Demuxer, Fmp4DemuxerConfig, Fmp4MuxEvent,
    Fmp4MuxSample, Fmp4Muxer, Fmp4MuxerConfig, MediaKind, TrackId, TrackInfo,
};
use proptest::prelude::*;

fn h264_track() -> TrackInfo {
    let mut t = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
    t.width = Some(1920);
    t.height = Some(1080);
    t.extradata = CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1E])],
        pps: vec![Bytes::from_static(&[0x68, 0xCE, 0x38])],
        avcc: Some(Bytes::from_static(&[
            0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
            0x03, 0x68, 0xCE, 0x38,
        ])),
    };
    t
}

fn aac_track() -> TrackInfo {
    let mut t = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 44_100);
    t.sample_rate = Some(44_100);
    t.channels = Some(2);
    t.extradata = CodecExtradata::AAC {
        asc: Bytes::from_static(&[0x12, 0x10]),
    };
    t
}

proptest! {
    /// Mux then demux roundtrip preserves frame count and keyframe flags.
    #[test]
    fn prop_mux_demux_roundtrip_frame_count(
        frame_count in 1usize..16,
        payload_len in 1usize..256,
    ) {
        let tracks = vec![h264_track()];
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks);
        let init_events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &init_events[0] else { panic!() };

        let samples: Vec<Fmp4MuxSample> = (0..frame_count)
            .map(|i| Fmp4MuxSample {
                track_id: 1,
                dts_us: i as i64 * 33_333,
                pts_us: i as i64 * 33_333 + 10_000,
                is_keyframe: i == 0,
                data: Bytes::from(vec![0x65u8; payload_len]),
            })
            .collect();

        let seg_events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data: seg_data, .. } = &seg_events[0] else { panic!() };

        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        demuxer.push(init_data);
        let frame_events = demuxer.push(seg_data);

        let frames: Vec<_> = frame_events
            .iter()
            .filter(|e| matches!(e, Fmp4DemuxEvent::Frame { .. }))
            .collect();
        prop_assert_eq!(frames.len(), frame_count);

        // First frame should be keyframe
        if let Fmp4DemuxEvent::Frame { keyframe, .. } = &frames[0] {
            prop_assert!(*keyframe);
        }
    }

    /// Arbitrary chunk splitting of demux input produces same result as single push.
    #[test]
    fn prop_chunk_split_invariant(
        split_points in proptest::collection::vec(1usize..100, 1..8),
    ) {
        let tracks = vec![h264_track(), aac_track()];
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks);
        let init_events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &init_events[0] else { panic!() };

        let samples = vec![
            Fmp4MuxSample { track_id: 1, dts_us: 0, pts_us: 33_333, is_keyframe: true, data: Bytes::from(vec![0x65; 50]) },
            Fmp4MuxSample { track_id: 2, dts_us: 0, pts_us: 0, is_keyframe: true, data: Bytes::from(vec![0xFF; 30]) },
        ];
        let seg_events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data: seg_data, .. } = &seg_events[0] else { panic!() };

        // Combine init + segment
        let mut full = Vec::new();
        full.extend_from_slice(init_data);
        full.extend_from_slice(seg_data);

        // Single push
        let mut demuxer_single = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        let single_events = demuxer_single.push(&full);
        let single_frames: Vec<_> = single_events
            .iter()
            .filter(|e| matches!(e, Fmp4DemuxEvent::Frame { .. }))
            .collect();

        // Chunked push
        let mut demuxer_chunked = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        let mut chunked_events = Vec::new();
        let mut offset = 0;
        for &sp in &split_points {
            let end = (offset + sp).min(full.len());
            if offset >= full.len() { break; }
            chunked_events.extend(demuxer_chunked.push(&full[offset..end]));
            offset = end;
        }
        if offset < full.len() {
            chunked_events.extend(demuxer_chunked.push(&full[offset..]));
        }
        let chunked_frames: Vec<_> = chunked_events
            .iter()
            .filter(|e| matches!(e, Fmp4DemuxEvent::Frame { .. }))
            .collect();

        prop_assert_eq!(single_frames.len(), chunked_frames.len(),
            "single={} chunked={}", single_frames.len(), chunked_frames.len());
    }

    /// Multi-track mux produces unique track IDs in demuxed TrackInfo.
    #[test]
    fn prop_multi_track_ids_unique(
        num_audio_tracks in 1usize..4,
    ) {
        let mut tracks = vec![h264_track()];
        for i in 0..num_audio_tracks {
            let mut t = TrackInfo::new(TrackId(10 + i as u32), MediaKind::Audio, CodecId::AAC, 44_100);
            t.sample_rate = Some(44_100);
            t.channels = Some(2);
            t.extradata = CodecExtradata::AAC { asc: Bytes::from_static(&[0x12, 0x10]) };
            tracks.push(t);
        }

        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks);
        let init_events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &init_events[0] else { panic!() };

        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        let events = demuxer.push(init_data);

        let track_info = events.iter().find_map(|e| {
            if let Fmp4DemuxEvent::TrackInfo(t) = e { Some(t) } else { None }
        });
        prop_assert!(track_info.is_some());
        let tracks_out = track_info.unwrap();
        prop_assert_eq!(tracks_out.len(), 1 + num_audio_tracks);

        // All track IDs unique
        let mut ids: Vec<u32> = tracks_out.iter().map(|t| t.track_id).collect();
        ids.sort();
        ids.dedup();
        prop_assert_eq!(ids.len(), 1 + num_audio_tracks);
    }

    /// Empty samples produce no media segment output.
    #[test]
    fn prop_empty_samples_no_output(_dummy in 0u8..1) {
        let tracks = vec![h264_track()];
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks);
        let events = muxer.write_segment(&[]);
        prop_assert!(events.is_empty());
    }

    /// sidx referenced_size matches actual moof+mdat size.
    #[test]
    fn prop_sidx_referenced_size_correct(
        frame_count in 1usize..8,
        payload_len in 1usize..128,
    ) {
        let tracks = vec![h264_track()];
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig {
            include_styp: true,
            include_sidx: true,
            ..Default::default()
        }, &tracks);

        let samples: Vec<Fmp4MuxSample> = (0..frame_count)
            .map(|i| Fmp4MuxSample {
                track_id: 1,
                dts_us: i as i64 * 33_333,
                pts_us: i as i64 * 33_333,
                is_keyframe: i == 0,
                data: Bytes::from(vec![0x65u8; payload_len]),
            })
            .collect();

        let seg_events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data, .. } = &seg_events[0] else { panic!() };

        // Find sidx box
        let styp_size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let sidx_size = u32::from_be_bytes([
            data[styp_size], data[styp_size + 1], data[styp_size + 2], data[styp_size + 3]
        ]) as usize;

        // Read referenced_size from sidx reference entry
        let ref_entry_offset = styp_size + 12 + 8 + 8 + 4 + 2 + 2;
        let ref_size = u32::from_be_bytes([
            data[ref_entry_offset], data[ref_entry_offset + 1],
            data[ref_entry_offset + 2], data[ref_entry_offset + 3],
        ]) as usize;

        let moof_mdat_actual = data.len() - styp_size - sidx_size;
        prop_assert_eq!(ref_size, moof_mdat_actual);
    }

    /// Multi-track segment: trun.data_offset correctly points into mdat for each track.
    #[test]
    fn prop_multi_traf_data_offset(
        video_payload_len in 1usize..64,
        audio_payload_len in 1usize..64,
    ) {
        let tracks = vec![h264_track(), aac_track()];
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks);
        let init_events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &init_events[0] else { panic!() };

        let video_data = vec![0x65u8; video_payload_len];
        let audio_data = vec![0xFFu8; audio_payload_len];
        let samples = vec![
            Fmp4MuxSample {
                track_id: 1, dts_us: 0, pts_us: 33_333,
                is_keyframe: true, data: Bytes::from(video_data.clone()),
            },
            Fmp4MuxSample {
                track_id: 2, dts_us: 0, pts_us: 0,
                is_keyframe: true, data: Bytes::from(audio_data.clone()),
            },
        ];
        let seg_events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data: seg_data, .. } = &seg_events[0] else { panic!() };

        // Demux and verify both frames are extracted correctly
        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        demuxer.push(init_data);
        let frame_events = demuxer.push(seg_data);

        let frames: Vec<_> = frame_events
            .iter()
            .filter_map(|e| {
                if let Fmp4DemuxEvent::Frame { track_id, data, .. } = e {
                    Some((*track_id, data.clone()))
                } else {
                    None
                }
            })
            .collect();

        prop_assert_eq!(frames.len(), 2);
        prop_assert_eq!(frames[0].0, 1); // video track
        prop_assert_eq!(frames[0].1.as_ref(), video_data.as_slice());
        prop_assert_eq!(frames[1].0, 2); // audio track
        prop_assert_eq!(frames[1].1.as_ref(), audio_data.as_slice());
    }

    /// Demuxed timestamps are monotonically non-decreasing per track.
    #[test]
    fn prop_timestamp_monotonicity(
        frame_count in 2usize..16,
        frame_duration_us in 10_000i64..100_000,
    ) {
        let tracks = vec![h264_track()];
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks);
        let init_events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &init_events[0] else { panic!() };

        let samples: Vec<Fmp4MuxSample> = (0..frame_count)
            .map(|i| Fmp4MuxSample {
                track_id: 1,
                dts_us: i as i64 * frame_duration_us,
                pts_us: i as i64 * frame_duration_us + frame_duration_us / 3,
                is_keyframe: i == 0,
                data: Bytes::from(vec![0x65u8; 10]),
            })
            .collect();

        let seg_events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data: seg_data, .. } = &seg_events[0] else { panic!() };

        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        demuxer.push(init_data);
        let frame_events = demuxer.push(seg_data);

        let dts_values: Vec<i64> = frame_events
            .iter()
            .filter_map(|e| {
                if let Fmp4DemuxEvent::Frame { dts_us, .. } = e {
                    Some(*dts_us)
                } else {
                    None
                }
            })
            .collect();

        prop_assert_eq!(dts_values.len(), frame_count);
        // DTS should be monotonically non-decreasing
        for i in 1..dts_values.len() {
            prop_assert!(dts_values[i] >= dts_values[i - 1],
                "DTS not monotonic: dts[{}]={} < dts[{}]={}",
                i, dts_values[i], i - 1, dts_values[i - 1]);
        }
    }
}
