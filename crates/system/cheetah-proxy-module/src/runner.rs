//! Proxy data-plane runners.
//!
//! 代理数据面运行器。

#![allow(clippy::too_many_arguments)]

use std::sync::Arc;

use cheetah_codec::MonoTime;
use cheetah_media_api::command::RetryPolicy;
use cheetah_media_api::error::MediaError;
use cheetah_media_api::event::{EventHeader, MediaEvent, MediaEventSender};
use cheetah_media_api::ids::ProxyId;
use cheetah_media_api::model::ProxyState;
use cheetah_sdk::{CancellationToken, RuntimeApi};
use cheetah_sdk::{
    ConnectorApi, ConnectorPullOptions, ConnectorPushOptions, DispatchResult, PublishLease,
    PublisherApi, PublisherSink, SdkError, SubscriberSource,
};
use futures::{pin_mut, select_biased, FutureExt};

use crate::registry::ProxyRegistry;

/// Run a pull proxy until completion or cancellation.
///
/// 运行拉流代理直到完成或被取消。
pub async fn run_pull(
    registry: ProxyRegistry,
    event_sender: Option<Arc<dyn MediaEventSender>>,
    connector_api: Arc<dyn ConnectorApi>,
    url: String,
    sink: Box<dyn PublisherSink>,
    lease: PublishLease,
    publisher_api: Arc<dyn PublisherApi>,
    proxy_id: ProxyId,
    retry_policy: RetryPolicy,
    cancel: CancellationToken,
    runtime_api: Arc<dyn RuntimeApi>,
) {
    let result = run_pull_inner(
        &registry,
        &event_sender,
        connector_api.as_ref(),
        &url,
        sink.as_ref(),
        &proxy_id,
        &retry_policy,
        &cancel,
        runtime_api.as_ref(),
    )
    .await;

    let _ = sink.close();
    let _ = publisher_api.release_publisher(&lease).await;

    let (state, last_error) = match result {
        Ok(()) => (ProxyState::Stopped, None),
        Err(e) => (ProxyState::Failed, Some(e.to_string())),
    };
    update_proxy_state(&registry, &event_sender, &proxy_id, state, last_error);
}

async fn run_pull_inner(
    registry: &ProxyRegistry,
    event_sender: &Option<Arc<dyn MediaEventSender>>,
    connector_api: &dyn ConnectorApi,
    url: &str,
    sink: &dyn PublisherSink,
    proxy_id: &ProxyId,
    retry_policy: &RetryPolicy,
    cancel: &CancellationToken,
    runtime_api: &dyn RuntimeApi,
) -> Result<(), MediaError> {
    if cancel.is_cancelled() {
        return Ok(());
    }

    let mut retry_count: u32 = 0;
    let mut last_error: Option<String> = None;

    loop {
        update_proxy_state(
            registry,
            event_sender,
            proxy_id,
            ProxyState::Connecting,
            last_error.clone(),
        );

        let mut source = match open_pull_source(connector_api, url, cancel).await {
            Some(Ok(s)) => s,
            Some(Err(e)) => {
                last_error = Some(e.to_string());
                if retry_count >= retry_policy.max_retries {
                    update_proxy_state(
                        registry,
                        event_sender,
                        proxy_id,
                        ProxyState::Failed,
                        last_error,
                    );
                    return Err(e);
                }
                retry_count += 1;
                update_proxy_state(
                    registry,
                    event_sender,
                    proxy_id,
                    ProxyState::Reconnecting,
                    last_error.clone(),
                );
                if sleep_or_cancel(runtime_api, retry_count, retry_policy, cancel).await {
                    return Ok(());
                }
                continue;
            }
            None => {
                return Ok(());
            }
        };

        let tracks = source.tracks();
        if !tracks.is_empty() {
            let _ = sink.update_tracks(tracks);
        }
        update_proxy_state(
            registry,
            event_sender,
            proxy_id,
            ProxyState::Connected,
            None,
        );

        let frame_err = forward_frames(&mut source, sink, cancel).await;
        let _ = source.close().await;

        match frame_err {
            None => {
                update_proxy_state(registry, event_sender, proxy_id, ProxyState::Stopped, None);
                return Ok(());
            }
            Some(e) => {
                last_error = Some(e.to_string());
                if retry_count >= retry_policy.max_retries {
                    update_proxy_state(
                        registry,
                        event_sender,
                        proxy_id,
                        ProxyState::Failed,
                        last_error,
                    );
                    return Err(e);
                }
                retry_count += 1;
                update_proxy_state(
                    registry,
                    event_sender,
                    proxy_id,
                    ProxyState::Reconnecting,
                    last_error.clone(),
                );
                if sleep_or_cancel(runtime_api, retry_count, retry_policy, cancel).await {
                    update_proxy_state(registry, event_sender, proxy_id, ProxyState::Stopped, None);
                    return Ok(());
                }
            }
        }
    }
}

async fn open_pull_source(
    connector_api: &dyn ConnectorApi,
    url: &str,
    cancel: &CancellationToken,
) -> Option<Result<Box<dyn SubscriberSource>, MediaError>> {
    let open_fut = connector_api
        .open_pull(url, ConnectorPullOptions::default())
        .fuse();
    let cancel_fut = cancel.cancelled().fuse();
    pin_mut!(open_fut, cancel_fut);

    select_biased! {
        _ = cancel_fut => None,
        res = open_fut => Some(res.map_err(map_sdk_error)),
    }
}

/// Run a push proxy until completion or cancellation.
///
/// 运行推流代理直到完成或被取消。
pub async fn run_push(
    registry: ProxyRegistry,
    event_sender: Option<Arc<dyn MediaEventSender>>,
    connector_api: Arc<dyn ConnectorApi>,
    url: String,
    mut source: Box<dyn SubscriberSource>,
    proxy_id: ProxyId,
    retry_policy: RetryPolicy,
    cancel: CancellationToken,
    runtime_api: Arc<dyn RuntimeApi>,
) {
    let result = run_push_inner(
        &registry,
        &event_sender,
        connector_api.as_ref(),
        &url,
        &mut source,
        &proxy_id,
        &retry_policy,
        &cancel,
        runtime_api.as_ref(),
    )
    .await;

    let _ = source.close().await;

    let (state, last_error) = match result {
        Ok(()) => (ProxyState::Stopped, None),
        Err(e) => (ProxyState::Failed, Some(e.to_string())),
    };
    update_proxy_state(&registry, &event_sender, &proxy_id, state, last_error);
}

async fn run_push_inner(
    registry: &ProxyRegistry,
    event_sender: &Option<Arc<dyn MediaEventSender>>,
    connector_api: &dyn ConnectorApi,
    url: &str,
    source: &mut Box<dyn SubscriberSource>,
    proxy_id: &ProxyId,
    retry_policy: &RetryPolicy,
    cancel: &CancellationToken,
    runtime_api: &dyn RuntimeApi,
) -> Result<(), MediaError> {
    if cancel.is_cancelled() {
        return Ok(());
    }

    let mut sink: Option<Box<dyn PublisherSink>> = None;
    let mut retry_count: u32 = 0;
    let mut last_error: Option<String> = None;

    loop {
        update_proxy_state(
            registry,
            event_sender,
            proxy_id,
            ProxyState::Connecting,
            last_error.clone(),
        );

        if sink.is_none() {
            match open_push_sink(connector_api, url, cancel).await {
                Some(Ok(s)) => sink = Some(s),
                Some(Err(e)) => {
                    last_error = Some(e.to_string());
                    if retry_count >= retry_policy.max_retries {
                        update_proxy_state(
                            registry,
                            event_sender,
                            proxy_id,
                            ProxyState::Failed,
                            last_error,
                        );
                        return Err(e);
                    }
                    retry_count += 1;
                    update_proxy_state(
                        registry,
                        event_sender,
                        proxy_id,
                        ProxyState::Reconnecting,
                        last_error.clone(),
                    );
                    if sleep_or_cancel(runtime_api, retry_count, retry_policy, cancel).await {
                        return Ok(());
                    }
                    continue;
                }
                None => {
                    return Ok(());
                }
            }
        }

        let s = sink.as_deref().unwrap();
        let tracks = source.tracks();
        if !tracks.is_empty() {
            let _ = s.update_tracks(tracks);
        }
        update_proxy_state(
            registry,
            event_sender,
            proxy_id,
            ProxyState::Connected,
            None,
        );

        let frame_err = forward_frames(source, s, cancel).await;

        if frame_err.is_some() {
            if let Some(old) = sink.take() {
                let _ = old.close();
            }
        }

        match frame_err {
            None => {
                update_proxy_state(registry, event_sender, proxy_id, ProxyState::Stopped, None);
                return Ok(());
            }
            Some(e) => {
                last_error = Some(e.to_string());
                if retry_count >= retry_policy.max_retries {
                    update_proxy_state(
                        registry,
                        event_sender,
                        proxy_id,
                        ProxyState::Failed,
                        last_error,
                    );
                    return Err(e);
                }
                retry_count += 1;
                update_proxy_state(
                    registry,
                    event_sender,
                    proxy_id,
                    ProxyState::Reconnecting,
                    last_error.clone(),
                );
                if sleep_or_cancel(runtime_api, retry_count, retry_policy, cancel).await {
                    update_proxy_state(registry, event_sender, proxy_id, ProxyState::Stopped, None);
                    return Ok(());
                }
            }
        }
    }
}

async fn open_push_sink(
    connector_api: &dyn ConnectorApi,
    url: &str,
    cancel: &CancellationToken,
) -> Option<Result<Box<dyn PublisherSink>, MediaError>> {
    let open_fut = connector_api
        .open_push(url, ConnectorPushOptions::default())
        .fuse();
    let cancel_fut = cancel.cancelled().fuse();
    pin_mut!(open_fut, cancel_fut);

    select_biased! {
        _ = cancel_fut => None,
        res = open_fut => Some(res.map_err(map_sdk_error)),
    }
}

async fn forward_frames(
    source: &mut Box<dyn SubscriberSource>,
    sink: &dyn PublisherSink,
    cancel: &CancellationToken,
) -> Option<MediaError> {
    loop {
        if cancel.is_cancelled() {
            return None;
        }

        let recv_fut = source.recv().fuse();
        let cancel_fut = cancel.cancelled().fuse();
        pin_mut!(recv_fut, cancel_fut);

        let next = select_biased! {
            _ = cancel_fut => return None,
            frame = recv_fut => frame,
        };

        match next {
            Ok(Some(frame)) => match sink.push_frame(frame) {
                Ok(DispatchResult::Accepted | DispatchResult::DroppedByPolicy) => {}
                Ok(DispatchResult::RejectedClosed) => {
                    return Some(MediaError::unavailable("sink closed"));
                }
                Err(e) => return Some(map_sdk_error(e)),
            },
            Ok(None) => return Some(MediaError::unavailable("source closed")),
            Err(e) => return Some(map_sdk_error(e)),
        }
    }
}

async fn sleep_or_cancel(
    runtime_api: &dyn RuntimeApi,
    retry_count: u32,
    policy: &RetryPolicy,
    cancel: &CancellationToken,
) -> bool {
    let delay_ms = backoff_ms(retry_count, policy);
    if delay_ms == 0 {
        return cancel.is_cancelled();
    }

    let now = runtime_api.now().as_micros();
    let deadline_micros = now.saturating_add(delay_ms.saturating_mul(1_000));
    let deadline = MonoTime::from_micros(deadline_micros);

    let mut timer = runtime_api.sleep_until(deadline);
    let timer_fut = timer.wait().fuse();
    let cancel_fut = cancel.cancelled().fuse();
    pin_mut!(timer_fut, cancel_fut);

    select_biased! {
        _ = timer_fut => false,
        _ = cancel_fut => true,
    }
}

fn backoff_ms(retry_count: u32, policy: &RetryPolicy) -> u64 {
    let base = policy.retry_delay_ms;
    let exp = 1u64
        .checked_shl(retry_count.saturating_sub(1).min(60))
        .unwrap_or(u64::MAX);
    let delay = base.saturating_mul(exp);
    delay.min(policy.max_retry_delay_ms)
}

fn update_proxy_state(
    registry: &ProxyRegistry,
    event_sender: &Option<Arc<dyn MediaEventSender>>,
    proxy_id: &ProxyId,
    state: ProxyState,
    last_error: Option<String>,
) {
    registry.update_state(proxy_id, state, last_error.clone());
    emit_proxy_state(registry, event_sender, proxy_id, state, last_error);
}

fn emit_proxy_state(
    registry: &ProxyRegistry,
    event_sender: &Option<Arc<dyn MediaEventSender>>,
    proxy_id: &ProxyId,
    state: ProxyState,
    last_error: Option<String>,
) {
    if let Some(sender) = event_sender.as_ref() {
        if let Some(info) = registry.get(proxy_id) {
            let header = EventHeader {
                event_id: crate::media_provider::generate_id(),
                occurred_at: now_ms(),
                sequence: None,
                media_key: Some(info.destination.clone()),
                source: info.source.clone(),
                correlation_id: None,
            };
            let _ = sender.send(MediaEvent::ProxyStateChanged(
                cheetah_media_api::event::ProxyStateChanged {
                    header,
                    proxy_id: info.proxy_id,
                    state,
                    last_error,
                },
            ));
        }
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn map_sdk_error(e: SdkError) -> MediaError {
    match e {
        SdkError::InvalidArgument(m) => MediaError::invalid_argument(m),
        SdkError::NotFound(m) => MediaError::not_found(m),
        SdkError::AlreadyExists(m) => MediaError::already_exists(m),
        SdkError::Conflict(m) => MediaError::conflict(m),
        SdkError::Unavailable(m) => MediaError::unavailable(m),
        SdkError::Internal(m) => MediaError::internal(m),
    }
}
