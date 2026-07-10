//! Capture fixture replay tests for `RtmpCore`.
//!
//! These tests load real RTMP byte captures (recorded from pcap files) and feed
//! them into the Sans-I/O `RtmpCore` state machine under various transport views.
//! The goal is to verify that the core remains bounded and that standard publish
//! captures still connect, publish, and emit monotonic media timestamps.
//!
//! `RtmpCore` 的抓包 fixture 回放测试。
//!
//! 这些测试加载真实的 RTMP 字节捕获（从 pcap 文件记录），并在不同传输视图下喂入 Sans-I/O 状态机 `RtmpCore`。
//! 目标是校验 core 在传输层被扰动时仍然保持有界，并且标准发布捕获仍能保持连接、发布以及单调媒体时间戳。

#[path = "support/capture_fixture.rs"]
mod capture_fixture;

use std::sync::OnceLock;

use bytes::Bytes;
use capture_fixture::{
    build_transport_view, load_capture_fixtures, CaptureFixture, TransportView, TransportViewKind,
};
use cheetah_rtmp_core::{
    CoreInput, CoreOutput, RtmpCore, RtmpCoreCommand, RtmpCoreError, RtmpEvent, RtmpMediaType,
};
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

const MANIFEST: &str = include_str!("testdata/rtmp-capture/manifest.tsv");

/// Summary of a replayed capture session.
///
/// Used to assert the high-level outcomes after feeding a complete byte sequence
/// (possibly mutated) into the core.
///
/// 回放捕获会话后的汇总。
///
/// 用于在将完整字节序列（可能被变换）喂入 core 后断言高级结果。
#[derive(Debug, Default)]
struct ReplaySummary {
    connected: bool,
    publish_requested: bool,
    media_count: usize,
    timestamps_monotonic: bool,
}

/// Lazy-loaded committed capture fixtures shared across all property tests.
///
/// 所有属性测试共享的延迟加载已提交捕获 fixtures。
static FIXTURES: OnceLock<Vec<CaptureFixture>> = OnceLock::new();

/// Return the cached set of capture fixtures.
///
/// 返回缓存的捕获 fixture 集合。
fn fixtures() -> &'static [CaptureFixture] {
    FIXTURES
        .get_or_init(|| {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/testdata/rtmp-capture");
            load_capture_fixtures(&root, MANIFEST).expect("committed capture fixtures should load")
        })
        .as_slice()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Verify that every transport view produces a bounded input set and that the
    /// replay either fails gracefully or preserves monotonic timestamps.
    ///
    /// For standard publish fixtures with pristine records, the replay must also
    /// connect, request publish, and emit the expected minimum number of media events.
    ///
    /// 校验每种传输视图产生有界输入集，并且回放要么优雅失败，要么保持时间戳单调。
    ///
    /// 对于 pristine 记录的标准发布 fixture，回放还必须连接、请求发布并发出期望的最小媒体事件数。
    #[test]
    fn capture_transport_views_are_bounded(
        case_index in any::<usize>(),
        view_index in 0u8..8,
        chunk_size in 64usize..=4096,
        truncation_point in 1usize..=262_144,
        repeat_count in 1usize..=3,
        drop_step in 2usize..=16,
    ) {
        let fixtures = fixtures();
        prop_assert!(!fixtures.is_empty());

        let fixture = &fixtures[case_index % fixtures.len()];
        let view_kind = TransportViewKind::from_index(view_index)
            .ok_or_else(|| TestCaseError::fail(format!("invalid view index {view_index}")))?;
        let view = TransportView {
            kind: view_kind,
            chunk_size,
            truncation_point,
            repeat_count,
            drop_step,
        };
        let inputs = build_transport_view(&fixture.records, view);
        let total_bytes: usize = fixture.records.iter().map(Vec::len).sum();
        let max_inputs = total_bytes
            .div_ceil(chunk_size.max(1))
            .saturating_add(fixture.records.len().saturating_mul(repeat_count + 2))
            .saturating_add(4);
        prop_assert!(
            inputs.len() <= max_inputs,
            "{} {:?} produced an unbounded input view",
            fixture.row.case,
            view_kind
        );

        let replay = replay_capture_inputs(inputs);
        if fixture.is_standard_publish() && view_kind == TransportViewKind::PristineRecords {
            let summary = replay.map_err(|err| {
                TestCaseError::fail(format!("{} pristine replay failed: {err}", fixture.row.case))
            })?;
            prop_assert!(summary.connected, "{} should connect", fixture.row.case);
            prop_assert!(summary.publish_requested, "{} should publish", fixture.row.case);
            prop_assert!(
                summary.media_count >= fixture.row.expect_media_min,
                "{} should emit at least {} media events, got {}",
                fixture.row.case,
                fixture.row.expect_media_min,
                summary.media_count
            );
            prop_assert!(summary.timestamps_monotonic, "{} timestamps should be monotonic", fixture.row.case);
        } else if let Ok(summary) = replay {
            prop_assert!(
                summary.timestamps_monotonic,
                "{} {:?} successful replay should keep emitted timestamps monotonic",
                fixture.row.case,
                view_kind
            );
        }
    }
}

/// Verify that standard publish fixtures keep strong assertions under the
/// pristine transport view.
///
/// This is a deterministic regression test in addition to the property test above.
///
/// 校验标准发布 fixture 在 pristine 传输视图下保持强断言。
///
/// 这是上述属性测试之外的一个确定性回归测试。
#[test]
fn standard_pristine_capture_fixtures_keep_strong_assertions() {
    for fixture in fixtures()
        .iter()
        .filter(|fixture| fixture.is_standard_publish())
    {
        let inputs = build_transport_view(
            &fixture.records,
            TransportView {
                kind: TransportViewKind::PristineRecords,
                chunk_size: 128,
                truncation_point: 0,
                repeat_count: 1,
                drop_step: 5,
            },
        );
        let summary = replay_capture_inputs(inputs)
            .unwrap_or_else(|err| panic!("{} pristine replay failed: {err}", fixture.row.case));
        assert!(summary.connected, "{} should connect", fixture.row.case);
        assert!(
            summary.publish_requested,
            "{} should publish",
            fixture.row.case
        );
        assert!(
            summary.media_count >= fixture.row.expect_media_min,
            "{} should emit at least {} media events, got {}",
            fixture.row.case,
            fixture.row.expect_media_min,
            summary.media_count
        );
        assert!(
            summary.timestamps_monotonic,
            "{} timestamps should be monotonic",
            fixture.row.case
        );
    }
}

/// Replay a sequence of transport byte chunks through `RtmpCore` and return a summary.
///
/// The core handles handshakes, connect, and publish commands. When a publish is
/// requested, the test accepts it so that media events can be emitted.
///
/// 通过 `RtmpCore` 回放传输字节块序列并返回汇总。
///
/// core 处理握手、连接和发布命令。当请求发布时，测试会接受它，以便发出媒体事件。
fn replay_capture_inputs(inputs: Vec<Bytes>) -> Result<ReplaySummary, RtmpCoreError> {
    let mut core = RtmpCore::new();
    let mut summary = ReplaySummary {
        timestamps_monotonic: true,
        ..ReplaySummary::default()
    };
    let mut publish_accepted = false;
    let mut last_audio = None;
    let mut last_video = None;
    let mut last_data = None;

    for input in inputs {
        let outputs = core.handle_input(CoreInput::Bytes(input))?;
        handle_outputs(
            &mut core,
            outputs,
            &mut summary,
            &mut publish_accepted,
            &mut last_audio,
            &mut last_video,
            &mut last_data,
        )?;
    }

    Ok(summary)
}

/// Process `CoreOutput` events and update the replay summary.
///
/// Accepts the first `PublishRequested` event and recursively handles any outputs
/// produced by `AcceptPublish`. Media timestamps are tracked per type to assert
/// monotonicity.
///
/// 处理 `CoreOutput` 事件并更新回放汇总。
///
/// 接受第一个 `PublishRequested` 事件，并递归处理 `AcceptPublish` 产生的输出。
/// 按类型跟踪媒体时间戳以断言单调性。
fn handle_outputs(
    core: &mut RtmpCore,
    outputs: Vec<CoreOutput>,
    summary: &mut ReplaySummary,
    publish_accepted: &mut bool,
    last_audio: &mut Option<u32>,
    last_video: &mut Option<u32>,
    last_data: &mut Option<u32>,
) -> Result<(), RtmpCoreError> {
    for output in outputs {
        let CoreOutput::Event(event) = output else {
            continue;
        };
        match event {
            RtmpEvent::Connected { .. } => summary.connected = true,
            RtmpEvent::PublishRequested { stream_id, .. } => {
                summary.publish_requested = true;
                if !*publish_accepted {
                    *publish_accepted = true;
                    let accept_outputs =
                        core.handle_input(CoreInput::Command(RtmpCoreCommand::AcceptPublish {
                            stream_id,
                        }))?;
                    handle_outputs(
                        core,
                        accept_outputs,
                        summary,
                        publish_accepted,
                        last_audio,
                        last_video,
                        last_data,
                    )?;
                }
            }
            RtmpEvent::MediaData {
                timestamp_ms,
                media_type,
                ..
            } => {
                summary.media_count += 1;
                update_timestamp_monotonic(
                    summary,
                    match media_type {
                        RtmpMediaType::Audio => last_audio,
                        RtmpMediaType::Video => last_video,
                        RtmpMediaType::Data => last_data,
                    },
                    timestamp_ms,
                );
            }
            _ => {}
        }
    }
    Ok(())
}

/// Update the per-type last timestamp and flag monotonicity violations.
///
/// 更新每类型的最近时间戳，并标记单调性违规。
fn update_timestamp_monotonic(
    summary: &mut ReplaySummary,
    last: &mut Option<u32>,
    timestamp_ms: u32,
) {
    if let Some(previous) = *last {
        summary.timestamps_monotonic &= previous <= timestamp_ms;
    }
    *last = Some(timestamp_ms);
}
