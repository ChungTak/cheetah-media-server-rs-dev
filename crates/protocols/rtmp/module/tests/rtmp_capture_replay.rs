mod support {
    pub mod capture_fixture;
    pub mod rtmp_test_harness;
}

use std::time::Duration;

use support::capture_fixture::{
    build_fault_chunks, module_health_fault_cases, play_acceptance_cases, probe_publish_cases,
    standard_publish_cases, CaptureFaultKind,
};
use support::rtmp_test_harness::{RawReplayMode, RtmpTestHarness};

#[tokio::test(flavor = "current_thread")]
async fn standard_capture_record_replay_publishes_to_engine() {
    for case in standard_publish_cases() {
        let harness = RtmpTestHarness::start().await;

        harness
            .replay_raw_publish(&case, RawReplayMode::RecordBoundaries)
            .await;
        let snapshot = harness
            .wait_for_active_published_stream(Duration::from_secs(4))
            .await;

        assert!(
            snapshot.publisher_active,
            "expected active publisher after raw record replay for {}",
            case.name
        );
        assert!(
            !snapshot.tracks.is_empty(),
            "expected announced tracks after raw record replay for {}",
            case.name
        );

        harness.stop().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn standard_capture_coalesced_replay_publishes_to_engine() {
    for case in standard_publish_cases() {
        let harness = RtmpTestHarness::start().await;

        harness
            .replay_raw_publish(&case, RawReplayMode::Coalesced)
            .await;
        let snapshot = harness
            .wait_for_active_published_stream(Duration::from_secs(4))
            .await;

        assert!(
            snapshot.publisher_active,
            "expected active publisher after coalesced raw replay for {}",
            case.name
        );
        assert!(
            !snapshot.tracks.is_empty(),
            "expected announced tracks after coalesced raw replay for {}",
            case.name
        );

        harness.stop().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn standard_capture_raw_publish_can_be_played_with_monotonic_timestamps() {
    for case in play_acceptance_cases() {
        let harness = RtmpTestHarness::start().await;

        let mut publisher = harness.start_raw_publish_prefix(&case, 24).await;
        let snapshot = harness
            .wait_for_active_published_stream(Duration::from_secs(4))
            .await;

        let play_stage = format!(
            "{} stream={} tracks={:?}",
            case.name, snapshot.key, snapshot.tracks
        );
        let mut player = harness.start_play_client(&snapshot.key);
        harness.wait_for_playing(&mut player, &play_stage).await;
        publisher.finish_remaining().await;
        let media = harness
            .collect_play_media(&mut player, &case, Duration::from_secs(4))
            .await;

        if case.expect_video {
            assert!(
                !media.video_timestamps_ms.is_empty(),
                "expected at least one video media event while playing {}",
                case.name
            );
            assert_monotonic(&media.video_timestamps_ms, case.name, "video");
        }
        if case.expect_audio {
            assert!(
                !media.audio_timestamps_ms.is_empty(),
                "expected at least one audio media event while playing {}",
                case.name
            );
            assert_monotonic(&media.audio_timestamps_ms, case.name, "audio");
        }
        if media.saw_video_config && media.saw_video_coded {
            assert!(
                media.video_coded_after_playing,
                "coded video must not be observed before play reaches Playing for {}",
                case.name
            );
        }

        player.shutdown();
        player.wait().await.expect("wait rtmp play client");
        publisher.shutdown().await;
        harness.stop().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn probe_capture_raw_replay_keeps_module_running() {
    for case in probe_publish_cases() {
        let harness = RtmpTestHarness::start().await;

        harness
            .replay_raw_publish(&case, RawReplayMode::RecordBoundaries)
            .await;
        harness.assert_rtmp_running_and_healthy().await;

        harness.stop().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn capture_transport_faults_keep_module_and_engine_healthy() {
    for case in module_health_fault_cases() {
        for fault in [
            CaptureFaultKind::PrefixTruncated,
            CaptureFaultKind::DroppedEveryNth,
            CaptureFaultKind::ReorderedAdjacent,
        ] {
            let harness = RtmpTestHarness::start().await;
            let chunks = build_fault_chunks(&case, fault);

            harness.replay_raw_fault_chunks(case.name, chunks).await;
            harness.assert_rtmp_running_and_healthy().await;

            harness.stop().await;
        }
    }
}

fn assert_monotonic(timestamps: &[u32], case_name: &str, media_kind: &str) {
    for window in timestamps.windows(2) {
        assert!(
            window[1] >= window[0],
            "{media_kind} timestamps must be monotonic for {case_name}: {timestamps:?}"
        );
    }
}
