use super::play::{send_play_rtcp_packet, PlayRtcpPacket, PlayRtcpSendError};
use super::*;
/// `cleanup_connection` function.
/// `cleanup_connection` 函数.
pub(super) async fn cleanup_connection(
    connection_id: RtspConnectionId,
    engine: &EngineContext,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    multicast: &Arc<MulticastSenderRegistry>,
) {
    cleanup_connection_with_config(connection_id, engine, sessions, multicast, 0).await;
}

/// `cleanup_connection_with_config` function.
/// `cleanup_connection_with_config` 函数.
pub(super) async fn cleanup_connection_with_config(
    connection_id: RtspConnectionId,
    engine: &EngineContext,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    multicast: &Arc<MulticastSenderRegistry>,
    continue_push_ms: u64,
) {
    let state = sessions.lock().remove(&connection_id);
    let Some(mut state) = state else {
        return;
    };

    cancel_pending_describe(&mut state);
    if let Some(mut publish) = state.publish.take() {
        flush_publish_video_reorder(&mut publish);
        publish.cancel.cancel();
        for join in publish.udp_task_handles.drain(..) {
            join.abort();
        }
        let _ = publish.sink.close();
        if continue_push_ms > 0 {
            // Delayed release: spawn a task that waits before releasing the publisher lease.
            // If a new publisher takes over the same stream key within the timeout,
            // the lease will already be released by the new publisher's ownership acquisition.
            let lease = publish.lease;
            let publisher_api = engine.publisher_api.clone();
            let runtime_api = engine.runtime_api.clone();
            let runtime_api_inner = runtime_api.clone();
            let _ = runtime_api.spawn(Box::pin(async move {
                let deadline = cheetah_codec::MonoTime::from_micros(
                    runtime_api_inner
                        .now()
                        .as_micros()
                        .saturating_add(continue_push_ms.saturating_mul(1000)),
                );
                runtime_api_inner.sleep_until(deadline).wait().await;
                let _ = publisher_api.release_publisher(&lease).await;
            }));
        } else {
            let _ = engine.publisher_api.release_publisher(&publish.lease).await;
        }
    }
    if let Some(play) = state.play.take() {
        play.cancel.cancel();
        play.join.abort();
    }
    let now_micros = runtime_unix_time_micros(&engine.runtime_api);
    for (track_id, track_state) in state.play_tracks {
        if let PlayTransport::UdpMulticast {
            stream_key,
            track_id: transport_track_id,
            ..
        } = track_state.transport
        {
            let release_track_id = if transport_track_id == track_id {
                track_id
            } else {
                transport_track_id
            };
            multicast.release(
                &engine.runtime_api,
                now_micros,
                connection_id,
                &stream_key,
                release_track_id,
            );
        }
    }
}

fn cancel_pending_describe(state: &mut RtspConnectionState) {
    if let Some(cancel) = state.describe_pending.take() {
        cancel.cancel();
    }
}

/// `send_play_rtcp_bye` function.
/// `send_play_rtcp_bye` 函数.
pub(super) async fn send_play_rtcp_bye(
    connection_id: RtspConnectionId,
    command_tx: &RtspCoreCommandSender,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
) {
    let bye_packets: Vec<(TrackId, u32, PlayTransport)> = {
        let guard = sessions.lock();
        let Some(state) = guard.get(&connection_id) else {
            return;
        };
        state
            .play_tracks
            .iter()
            .map(|(track_id, track)| (*track_id, track.ssrc, track.transport.clone()))
            .collect()
    };

    for (track_id, ssrc, transport) in bye_packets {
        if let Err(err) = send_play_rtcp_packet(
            command_tx,
            connection_id,
            &transport,
            PlayRtcpPacket::Bye {
                ssrc,
                reason: Some("teardown".to_string()),
            },
        )
        .await
        {
            match err {
                PlayRtcpSendError::Build { packet, detail } => {
                    warn!(
                        connection_id,
                        track_id = track_id.0,
                        "build play rtcp {packet} failed: {detail}"
                    );
                }
                PlayRtcpSendError::Send { packet } => {
                    warn!(
                        connection_id,
                        track_id = track_id.0,
                        "send play rtcp {packet} failed"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_pending_describe_cancels_and_clears_token() {
        let cancel = CancellationToken::new();
        let mut state = RtspConnectionState::new(7);
        state.describe_pending = Some(cancel.clone());

        cancel_pending_describe(&mut state);

        assert!(cancel.is_cancelled());
        assert!(state.describe_pending.is_none());
    }
}
