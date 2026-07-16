use std::collections::HashMap;
use std::sync::Arc;

use cheetah_media_api::command::*;
use cheetah_media_api::ids::*;
use cheetah_media_api::model::*;
use cheetah_sdk::MediaServices;

use crate::fake_support::{ctx, FakeMediaProvider};

/// Simulate an Apple HomeKit project that opens a subscriber with a specific output
/// schema, requests a keyframe, and closes the subscriber when the HAP session ends.
///
/// 模拟一个 Apple HomeKit 项目：使用指定输出视图打开订阅者、请求关键帧、HAP 会话结束时关闭订阅者。
async fn homekit_flow(services: MediaServices) {
    let ctx = ctx();
    let key = MediaKey::with_default_vhost("homekit", "accessory_1", Some(MediaSchema::Webrtc))
        .expect("valid HomeKit key");

    let online = services
        .control()
        .expect("control provider available")
        .is_media_online(&ctx, &key)
        .await
        .expect("query online state");
    assert_eq!(online, OnlineState::Online);

    let sub = services
        .publish_subscribe()
        .expect("publish/subscribe provider available")
        .open_subscriber(
            &ctx,
            SubscribeRequest {
                media_key: key.clone(),
                output_schema: MediaSchema::Webrtc,
                subscriber_kind: "homekit".to_string(),
                start_policy: "immediate".to_string(),
                protocol: "webrtc".to_string(),
                remote_endpoint: None,
                auth_context: HashMap::new(),
            },
        )
        .await
        .expect("open subscriber");
    assert_eq!(sub.media_key, key);
    assert_eq!(sub.output_schema, MediaSchema::Webrtc);

    services
        .control()
        .expect("control provider available")
        .request_keyframe(&ctx, &key)
        .await
        .expect("request keyframe");

    services
        .publish_subscribe()
        .expect("publish/subscribe provider available")
        .close_handle(&ctx, &sub.session_id, CloseReason::Normal)
        .await
        .expect("close subscriber");
}

#[tokio::test]
async fn homekit_can_complete_media_call_flow() {
    let services = MediaServices::unavailable();
    let provider = Arc::new(FakeMediaProvider::new());
    services.register_control(provider.clone());
    services.register_publish_subscribe(provider);

    homekit_flow(services).await;
}

/// HomeKit also exercises RTP/SRTP packetization bridge handles.
///
/// HomeKit 还会使用 RTP/SRTP packetization 桥接句柄。
#[tokio::test]
async fn homekit_can_subscribe_and_close_multiple_times() {
    let services = MediaServices::unavailable();
    let provider = Arc::new(FakeMediaProvider::new());
    services.register_control(provider.clone());
    services.register_publish_subscribe(provider);

    for i in 0..3 {
        let ctx = ctx();
        let key = MediaKey::with_default_vhost(
            "homekit",
            format!("accessory_{i}"),
            Some(MediaSchema::Webrtc),
        )
        .expect("valid HomeKit key");

        let sub = services
            .publish_subscribe()
            .expect("publish/subscribe provider available")
            .open_subscriber(
                &ctx,
                SubscribeRequest {
                    media_key: key.clone(),
                    output_schema: MediaSchema::Webrtc,
                    subscriber_kind: "homekit".to_string(),
                    start_policy: "immediate".to_string(),
                    protocol: "webrtc".to_string(),
                    remote_endpoint: None,
                    auth_context: HashMap::new(),
                },
            )
            .await
            .expect("open subscriber");

        services
            .publish_subscribe()
            .expect("publish/subscribe provider available")
            .close_handle(&ctx, &sub.session_id, CloseReason::Normal)
            .await
            .expect("close subscriber");
    }
}
