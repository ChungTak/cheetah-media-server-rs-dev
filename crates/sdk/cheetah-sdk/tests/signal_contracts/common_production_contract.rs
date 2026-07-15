//! Common failure and edge-case production contract tests.
//!
//! These tests run against a real Engine and verify error paths that are shared
//! across external signaling projects.
//!
//! 本测试针对真实 Engine，验证各外部信令项目共用的错误路径与边界场景。

use cheetah_media_api::command::MediaQuery;
use cheetah_media_api::ids::MediaKey;
use cheetah_media_api::model::OnlineState;
use cheetah_media_api::MediaControlApi;

use crate::production_support::{ctx, golden_key, media_facade, production_engine};

#[tokio::test(flavor = "current_thread")]
async fn unknown_stream_is_not_online() {
    let engine = production_engine().await;
    let facade = media_facade(&engine);

    let unknown = MediaKey::with_default_vhost("live", "nonexistent", None).unwrap();
    let online = facade
        .is_media_online(&ctx(), &unknown)
        .await
        .expect("is_media_online");
    assert!(
        matches!(online, OnlineState::Offline | OnlineState::Unknown),
        "unknown stream should be offline or unknown, got {online:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn media_list_can_filter_and_paginate() {
    let engine = production_engine().await;
    let facade = media_facade(&engine);

    let query = MediaQuery {
        app: Some("live".to_string()),
        stream: Some("golden".to_string()),
        ..Default::default()
    };
    let list = facade
        .get_media_list(&ctx(), query)
        .await
        .expect("get_media_list");
    assert!(!list.items.is_empty(), "golden stream should be found");
    assert_eq!(list.items[0].key, golden_key());
    assert_eq!(list.page, 1);
    assert!(list.total >= 1);
}
