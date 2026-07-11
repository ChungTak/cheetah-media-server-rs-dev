async fn run_play_session(
    ctx: EngineContext,
    config: SrtModuleConfig,
    driver: SrtDriverHandle,
    peer_id: SrtPeerId,
    stream_key: StreamKey,
    cancel: CancellationToken,
) {
    let Some(snapshot) = wait_for_stream(&ctx, &stream_key, &config, &cancel).await else {
        if cancel.is_cancelled() {
            return;
        }
        driver
            .send(SrtDriverCommand::Close {
                peer_id,
                reason: "reject:stream_not_found".to_string(),
            })
            .await;
        return;
    };

    let queue_capacity = config
        .egress
        .subscriber_queue_capacity
        .max(config.egress.bootstrap_max_frames.max(1));
    let mut subscriber = match ctx
        .subscriber_api
        .subscribe(
            stream_key.clone(),
            SubscriberOptions {
                queue_capacity,
                backpressure: config.egress.subscriber_backpressure,
                bootstrap_policy: BootstrapPolicy::live_tail(
                    config.egress.bootstrap_max_frames,
                    None,
                ),
                ..Default::default()
            },
        )
        .await
    {
        Ok(subscriber) => subscriber,
        Err(err) => {
            driver
                .send(SrtDriverCommand::Close {
                    peer_id,
                    reason: format!("subscribe failed: {err}"),
                })
                .await;
            return;
        }
    };

    let tracks = snapshot.tracks;
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    send_muxer_tables(&driver, peer_id, &mut muxer).await;

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let recv_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, recv_fut);
        let frame = select_biased! {
            _ = cancel_fut => break,
            frame = recv_fut => frame,
        };
        match frame {
            Ok(Some(frame)) => {
                for event in muxer.push_frame(frame.as_ref()) {
                    if let MpegTsMuxEvent::Packet(payload) = event {
                        driver
                            .send(SrtDriverCommand::SendPayload { peer_id, payload })
                            .await;
                    }
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    let _ = subscriber.close().await;
}

/// Wait for a stream to exist and its tracks to become ready for egress.
///
/// 等待流存在且其轨道准备好 egress。
async fn wait_for_stream(
    ctx: &EngineContext,
    stream_key: &StreamKey,
    config: &SrtModuleConfig,
    cancel: &CancellationToken,
) -> Option<cheetah_sdk::StreamSnapshot> {
    let stream_deadline = ctx.runtime_api.now().as_micros().saturating_add(
        config
            .egress
            .play_wait_source_timeout_ms
            .saturating_mul(1_000),
    );
    let mut track_deadline = None;
    loop {
        let now = ctx.runtime_api.now().as_micros();
        if cancel.is_cancelled() || now >= stream_deadline {
            return None;
        }
        if let Ok(Some(snapshot)) = ctx.stream_manager_api.get_stream(stream_key).await {
            if tracks_ready_for_egress(&snapshot.tracks) {
                return Some(snapshot);
            }
            let deadline = *track_deadline.get_or_insert_with(|| {
                now.saturating_add(config.egress.track_ready_timeout_ms.saturating_mul(1_000))
            });
            if now >= deadline {
                return None;
            }
        }
        let deadline = cheetah_codec::MonoTime::from_micros(
            ctx.runtime_api.now().as_micros().saturating_add(100_000),
        );
        let mut sleep = ctx.runtime_api.sleep_until(deadline);
        let cancel_fut = cancel.cancelled().fuse();
        let sleep_fut = sleep.wait().fuse();
        pin_mut!(cancel_fut, sleep_fut);
        select_biased! {
            _ = cancel_fut => return None,
            _ = sleep_fut => {}
        }
    }
}

/// Check that all tracks are non-empty and individually ready.
///
/// 检查所有轨道非空且各自就绪。
fn tracks_ready_for_egress(tracks: &[TrackInfo]) -> bool {
    !tracks.is_empty() && tracks.iter().all(TrackInfo::is_ready)
}

/// Send the MPEG-TS PAT/PMT tables before the first media packets.
///
/// 在第一个媒体包之前发送 MPEG-TS PAT/PMT 表。
async fn send_muxer_tables(driver: &SrtDriverHandle, peer_id: SrtPeerId, muxer: &mut MpegTsMuxer) {
    for event in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(payload) = event {
            driver
                .send(SrtDriverCommand::SendPayload { peer_id, payload })
                .await;
        }
    }
}
