//! ONVIF production contract tests.
//!
//! These tests run against a real Engine and verify control, snapshot, record,
//! and proxy flows with real providers.
//!
//! 本测试针对真实 Engine，验证控制、快照、录制与代理的真实 provider 流程。

use cheetah_media_api::command::{
    MediaQuery, ProxyQuery, PullProxyRequest, RecordTaskQuery, SnapshotQuery, SnapshotRequest,
    StartRecordRequest, StopRecordRequest,
};
use cheetah_media_api::model::{OnlineState, RecordTaskState, SnapshotState};
use cheetah_media_api::{MediaControlApi, ProxyApi, RecordApi, SnapshotApi};

use crate::production_support::{ctx, golden_key, media_facade, production_engine, wait_ms};

#[tokio::test(flavor = "current_thread")]
async fn onvif_can_query_media_take_snapshot_and_record() {
    let engine = production_engine().await;
    let facade = media_facade(&engine);

    let online = facade
        .is_media_online(&ctx(), &golden_key())
        .await
        .expect("is_media_online");
    assert_eq!(online, OnlineState::Online);

    let query = MediaQuery {
        app: Some("live".to_string()),
        stream: Some("golden".to_string()),
        ..Default::default()
    };
    let list = facade
        .get_media_list(&ctx(), query)
        .await
        .expect("get_media_list");
    assert!(
        !list.items.is_empty(),
        "golden stream should appear in list"
    );

    facade
        .request_keyframe(&ctx(), &golden_key())
        .await
        .expect("request_keyframe");

    let snap = facade
        .take_snapshot(
            &ctx(),
            SnapshotRequest {
                media_key: golden_key(),
                timeout_ms: 5000,
                format: "jpg".to_string(),
                quality: None,
                max_width: None,
                max_height: None,
                storage_policy: Default::default(),
                capture_policy: Default::default(),
            },
        )
        .await
        .expect("take_snapshot");
    assert_eq!(snap.state, SnapshotState::Completed);

    let snaps = facade
        .query_snapshots(
            &ctx(),
            SnapshotQuery {
                vhost: None,
                app: Some("live".to_string()),
                stream: Some("golden".to_string()),
                start_time_ms: None,
                end_time_ms: None,
                page: 1,
                page_size: 100,
            },
        )
        .await
        .expect("query_snapshots");
    assert_eq!(snaps.items.len(), 1, "one snapshot should be registered");

    let task = facade
        .start_record(
            &ctx(),
            StartRecordRequest {
                media_key: golden_key(),
                format: "mp4".to_string(),
                template: Default::default(),
                segment_duration_ms: None,
                max_segments: None,
                storage_policy: Default::default(),
                idempotency_key: None,
            },
        )
        .await
        .expect("start_record");
    assert_eq!(task.state, RecordTaskState::Running);

    wait_ms(300).await;

    let tasks = facade
        .query_record_tasks(
            &ctx(),
            RecordTaskQuery {
                vhost: None,
                app: Some("live".to_string()),
                stream: Some("golden".to_string()),
                state: None,
                page: 1,
                page_size: 100,
            },
        )
        .await
        .expect("query_record_tasks");
    assert!(!tasks.items.is_empty(), "record task should be listed");

    let stopped = facade
        .stop_record(
            &ctx(),
            StopRecordRequest {
                task_id: task.task_id,
            },
        )
        .await
        .expect("stop_record");
    assert_ne!(stopped.state, RecordTaskState::Running);
}

#[tokio::test(flavor = "current_thread")]
async fn onvif_proxy_rejects_internal_target_and_lists_empty() {
    let engine = production_engine().await;
    let facade = media_facade(&engine);

    // Real ProxyModule enforces SSRF policy: loopback targets are rejected
    // before any proxy is created, so no network access is required.
    let err = facade
        .create_pull_proxy(
            &ctx(),
            PullProxyRequest {
                source_url: "http://127.0.0.1:1/live/stream.flv".to_string(),
                destination: golden_key(),
                retry_policy: Default::default(),
                heartbeat_ms: None,
                timeout_ms: 1000,
                transcode_policy: Default::default(),
                output_policy: Default::default(),
                record_policy: None,
            },
        )
        .await
        .expect_err("create_pull_proxy should reject loopback address");
    assert!(
        err.to_string().contains("forbidden proxy target"),
        "unexpected error: {err}"
    );

    let proxies = facade
        .list_pull_proxies(&ctx(), ProxyQuery::default())
        .await
        .expect("list_pull_proxies");
    assert!(proxies.items.is_empty(), "no proxy should be registered");
}
