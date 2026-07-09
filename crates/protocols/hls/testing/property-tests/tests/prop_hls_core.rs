use bytes::Bytes;
use cheetah_codec::CodecId;
use cheetah_hls_core::{parse_hls_request, PlaylistBuilder, SegmentRing, TsMuxer};
use proptest::prelude::*;

// --- Playlist invariants ---

fn segment_duration() -> impl Strategy<Value = f64> {
    (500u64..10000).prop_map(|ms| ms as f64 / 1000.0)
}

proptest! {
    #[test]
    fn prop_media_playlist_starts_with_extm3u(
        durations in proptest::collection::vec(segment_duration(), 1..10)
    ) {
        let mut ring = SegmentRing::new(10);
        for (i, d) in durations.iter().enumerate() {
            ring.push(format!("seg_{i}"), *d, Bytes::from(vec![0u8; 188]), true);
        }
        let m3u8 = PlaylistBuilder::build_media(&ring, None);
        prop_assert!(m3u8.starts_with("#EXTM3U\n"));
    }

    #[test]
    fn prop_target_duration_gte_max_extinf(
        durations in proptest::collection::vec(segment_duration(), 1..10)
    ) {
        let mut ring = SegmentRing::new(10);
        for (i, d) in durations.iter().enumerate() {
            ring.push(format!("seg_{i}"), *d, Bytes::from(vec![0u8; 188]), true);
        }
        let m3u8 = PlaylistBuilder::build_media(&ring, None);

        // Parse TARGETDURATION
        let td_line = m3u8.lines().find(|l| l.starts_with("#EXT-X-TARGETDURATION:")).unwrap();
        let td: u64 = td_line.strip_prefix("#EXT-X-TARGETDURATION:").unwrap().parse().unwrap();

        // All EXTINF durations must be <= TARGETDURATION
        for line in m3u8.lines().filter(|l| l.starts_with("#EXTINF:")) {
            let dur_str = line.strip_prefix("#EXTINF:").unwrap().trim_end_matches(',');
            let dur: f64 = dur_str.parse().unwrap();
            prop_assert!(td as f64 >= dur, "TARGETDURATION {} < EXTINF {}", td, dur);
        }
    }

    #[test]
    fn prop_segment_count_matches_ring(
        count in 1usize..8
    ) {
        let mut ring = SegmentRing::new(10);
        for i in 0..count {
            ring.push(format!("seg_{i}"), 4.0, Bytes::from(vec![0u8; 188]), true);
        }
        let m3u8 = PlaylistBuilder::build_media(&ring, None);
        let extinf_count = m3u8.matches("#EXTINF:").count();
        prop_assert_eq!(extinf_count, count);
    }

    #[test]
    fn prop_no_endlist_for_live(
        count in 1usize..5
    ) {
        let mut ring = SegmentRing::new(5);
        for i in 0..count {
            ring.push(format!("seg_{i}"), 4.0, Bytes::from(vec![0u8; 188]), true);
        }
        let m3u8 = PlaylistBuilder::build_media(&ring, None);
        prop_assert!(!m3u8.contains("#EXT-X-ENDLIST"));
    }

    // --- TS muxer invariants ---

    #[test]
    fn prop_ts_output_188_aligned(
        payload_len in 1usize..2000,
        pts in 0u64..1_000_000
    ) {
        let mut muxer = TsMuxer::new(CodecId::H264, CodecId::AAC, false);
        muxer.write_pat_pmt();
        let data = vec![0xAA_u8; payload_len];
        muxer.write_video(&data, pts * 90, pts * 90, true);
        let segment = muxer.take_segment();
        prop_assert_eq!(segment.len() % 188, 0, "output not 188-aligned: {} bytes", segment.len());
    }

    #[test]
    fn prop_ts_all_sync_bytes(
        payload_len in 1usize..500,
        pts in 0u64..100_000
    ) {
        let mut muxer = TsMuxer::new(CodecId::H265, CodecId::G711A, true);
        muxer.write_pat_pmt();
        muxer.write_video(&vec![0xBB; payload_len], pts * 90, pts * 90, false);
        muxer.write_audio(&vec![0xCC; 50], pts * 90);
        let segment = muxer.take_segment();
        for (i, chunk) in segment.chunks(188).enumerate() {
            prop_assert_eq!(chunk[0], 0x47, "packet {} missing sync byte", i);
        }
    }

    // --- Request parser invariants ---

    #[test]
    fn prop_parse_never_panics(input in "\\PC{0,200}") {
        let _ = parse_hls_request(&input);
    }

    #[test]
    fn prop_valid_master_playlist_roundtrip(
        ns in "[a-z]{1,10}",
        stream in "[a-z0-9]{1,10}"
    ) {
        let url = format!("/{ns}/{stream}.m3u8");
        let result = parse_hls_request(&url);
        prop_assert!(result.is_ok());
    }
}
