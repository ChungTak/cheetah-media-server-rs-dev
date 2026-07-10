use super::*;

/// `RtspRequestDispatchCtx` data structure.
/// `RtspRequestDispatchCtx` 数据结构.
pub(super) struct RtspRequestDispatchCtx {
    /// `engine` field of type `EngineContext`.
    /// `engine` 字段，类型为 `EngineContext`.
    engine: EngineContext,
    /// `config` field of type `RtspModuleConfig`.
    /// `config` 字段，类型为 `RtspModuleConfig`.
    config: RtspModuleConfig,
    /// `command_tx` field of type `RtspCoreCommandSender`.
    /// `command_tx` 字段，类型为 `RtspCoreCommandSender`.
    command_tx: RtspCoreCommandSender,
    /// `sessions` field.
    /// `sessions` 字段.
    sessions: Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    /// `multicast` field.
    /// `multicast` 字段.
    multicast: Arc<MulticastSenderRegistry>,
    /// `module_cancel` field of type `CancellationToken`.
    /// `module_cancel` 字段，类型为 `CancellationToken`.
    module_cancel: CancellationToken,
}

/// `handle_driver_event` function.
/// `handle_driver_event` 函数.
pub(super) async fn handle_driver_event(
    event: DriverEvent,
    engine: &EngineContext,
    config: &RtspModuleConfig,
    command_tx: &RtspCoreCommandSender,
    sessions: Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    multicast: Arc<MulticastSenderRegistry>,
    module_cancel: CancellationToken,
) {
    match event {
        DriverEvent::ConnectionOpened {
            connection_id,
            peer,
        } => {
            let mut guard = sessions.lock();
            let state = guard
                .entry(connection_id)
                .or_insert_with(|| RtspConnectionState::new(connection_id));
            state.peer_addr = peer;
        }
        DriverEvent::ConnectionClosed { connection_id, .. } => {
            cleanup_connection_with_config(
                connection_id,
                engine,
                &sessions,
                &multicast,
                config.continue_push_ms,
            )
            .await;
        }
        DriverEvent::Core {
            connection_id,
            event,
        } => match event {
            RtspEvent::Request(req) => {
                let dispatch_ctx = RtspRequestDispatchCtx {
                    engine: engine.clone(),
                    config: config.clone(),
                    command_tx: command_tx.clone(),
                    sessions: sessions.clone(),
                    multicast: multicast.clone(),
                    module_cancel: module_cancel.clone(),
                };
                let should_spawn_waiting_setup = req.method == RtspMethod::Setup
                    && sessions
                        .lock()
                        .get(&connection_id)
                        .and_then(|state| state.describe_pending.clone())
                        .is_some();
                if should_spawn_waiting_setup {
                    let dispatch_ctx = RtspRequestDispatchCtx {
                        engine: engine.clone(),
                        config: config.clone(),
                        command_tx: command_tx.clone(),
                        sessions: sessions.clone(),
                        multicast: multicast.clone(),
                        module_cancel: module_cancel.clone(),
                    };
                    let runtime_api = engine.runtime_api.clone();
                    let _ = runtime_api.spawn(Box::pin(async move {
                        handle_rtsp_request(connection_id, req, dispatch_ctx).await;
                    }));
                } else {
                    handle_rtsp_request(connection_id, req, dispatch_ctx).await;
                }
            }
            RtspEvent::InterleavedFrame { channel, payload } => {
                handle_interleaved_frame(
                    connection_id,
                    channel,
                    payload,
                    command_tx,
                    &sessions,
                    &engine.runtime_api,
                )
                .await;
            }
            RtspEvent::PeerClosed => {
                cleanup_connection(connection_id, engine, &sessions, &multicast).await;
            }
        },
    }
}

/// `handle_rtsp_request` function.
/// `handle_rtsp_request` 函数.
pub(super) async fn handle_rtsp_request(
    connection_id: RtspConnectionId,
    req: RtspRequest,
    ctx: RtspRequestDispatchCtx,
) {
    let RtspRequestDispatchCtx {
        engine,
        config,
        command_tx,
        sessions,
        multicast,
        module_cancel,
    } = ctx;
    let cseq = req.cseq;
    if !sessions.lock().contains_key(&connection_id) {
        return;
    }
    if req.method == RtspMethod::Setup {
        let describe_pending = sessions
            .lock()
            .get(&connection_id)
            .and_then(|state| state.describe_pending.clone());
        if let Some(describe_pending) = describe_pending {
            describe_pending.cancelled().await;
            if !sessions.lock().contains_key(&connection_id) {
                return;
            }
        }
    }
    let now_unix_micros = runtime_unix_time_micros(&engine.runtime_api);
    if let Err(auth_error) =
        check_request_auth(connection_id, &req, &config, &sessions, now_unix_micros)
    {
        let digest_nonce = if config.auth.enabled && config.auth.allow_digest {
            Some(issue_digest_nonce(
                connection_id,
                &sessions,
                now_unix_micros,
            ))
        } else {
            None
        };
        let mut headers = build_www_authenticate_headers(&config, digest_nonce.as_deref());
        let body_msg = match &auth_error {
            AuthError::StaleNonce => {
                // Re-generate headers with stale=true hint
                headers.clear();
                if let Some(nonce) = digest_nonce.as_deref() {
                    headers.push((
                        "WWW-Authenticate".to_string(),
                        format!(
                            "Digest realm=\"{}\", nonce=\"{}\", algorithm=SHA-256, stale=true",
                            config.auth.realm, nonce
                        ),
                    ));
                    headers.push((
                        "WWW-Authenticate".to_string(),
                        format!(
                            "Digest realm=\"{}\", nonce=\"{}\", algorithm=MD5, stale=true",
                            config.auth.realm, nonce
                        ),
                    ));
                }
                "nonce expired"
            }
            AuthError::Rejected(msg) => *msg,
        };
        send_response(
            &command_tx,
            connection_id,
            cseq,
            401,
            "Unauthorized",
            headers,
            Bytes::from(body_msg),
        )
        .await;
        return;
    }

    let session_check = match req.method {
        RtspMethod::Setup => validate_request_session(connection_id, &req, &sessions, false),
        RtspMethod::Play
        | RtspMethod::Pause
        | RtspMethod::Record
        | RtspMethod::Teardown
        | RtspMethod::GetParameter
        | RtspMethod::SetParameter => {
            validate_request_session(connection_id, &req, &sessions, true)
        }
        _ => Ok(()),
    };
    if let Err((code, reason, message)) = session_check {
        send_response(
            &command_tx,
            connection_id,
            cseq,
            code,
            reason,
            Vec::new(),
            Bytes::from_static(message),
        )
        .await;
        return;
    }

    match req.method {
        RtspMethod::Get | RtspMethod::Post => {
            send_response(
                &command_tx,
                connection_id,
                cseq,
                405,
                "Method Not Allowed",
                vec![("Allow".to_string(), RTSP_PUBLIC_METHODS.to_string())],
                Bytes::from_static(
                    b"unsupported method on RTSP endpoint: use RTSP-over-HTTP tunnel entrypoint",
                ),
            )
            .await;
        }
        RtspMethod::Options => {
            send_response(
                &command_tx,
                connection_id,
                cseq,
                200,
                "OK",
                vec![("Public".to_string(), RTSP_PUBLIC_METHODS.to_string())],
                Bytes::new(),
            )
            .await;
        }
        RtspMethod::Announce => {
            handle_announce(connection_id, req, &engine, &config, &command_tx, sessions).await;
        }
        RtspMethod::Describe => {
            handle_describe(connection_id, req, &engine, &config, &command_tx, sessions).await;
        }
        RtspMethod::Setup => {
            handle_setup(
                connection_id,
                req,
                &engine,
                &config,
                &command_tx,
                sessions,
                multicast.clone(),
            )
            .await;
        }
        RtspMethod::Play => {
            let requested_range = match parse_request_range_scale_headers(&req) {
                Ok(value) => value,
                Err((code, reason, message)) => {
                    send_response(
                        &command_tx,
                        connection_id,
                        cseq,
                        code,
                        reason,
                        Vec::new(),
                        Bytes::from_static(message),
                    )
                    .await;
                    return;
                }
            };
            handle_play(
                connection_id,
                PlayRequestMeta {
                    cseq,
                    requested_range,
                },
                &engine,
                &config,
                &command_tx,
                sessions,
                multicast.clone(),
                module_cancel,
            )
            .await;
        }
        RtspMethod::Pause => {
            let requested_range = match parse_request_range_scale_headers(&req) {
                Ok(value) => value,
                Err((code, reason, message)) => {
                    send_response(
                        &command_tx,
                        connection_id,
                        cseq,
                        code,
                        reason,
                        Vec::new(),
                        Bytes::from_static(message),
                    )
                    .await;
                    return;
                }
            };
            handle_pause(
                connection_id,
                PauseRequestMeta {
                    cseq,
                    requested_range,
                },
                &command_tx,
                &sessions,
                config.session_timeout_secs,
            )
            .await;
        }
        RtspMethod::Record => {
            handle_record(
                connection_id,
                cseq,
                &command_tx,
                &sessions,
                config.session_timeout_secs,
                config.enable_mute_audio,
            )
            .await;
        }
        RtspMethod::Teardown => {
            send_basic_ok_with_session(
                connection_id,
                cseq,
                &command_tx,
                &sessions,
                config.session_timeout_secs,
            )
            .await;
            send_play_rtcp_bye(connection_id, &command_tx, &sessions).await;
            cleanup_connection(connection_id, &engine, &sessions, &multicast).await;
            let _ = command_tx.close_connection(connection_id).await;
        }
        RtspMethod::GetParameter => {
            handle_get_parameter(
                connection_id,
                req,
                &command_tx,
                &sessions,
                config.session_timeout_secs,
            )
            .await;
        }
        RtspMethod::SetParameter => {
            send_basic_ok_with_session(
                connection_id,
                cseq,
                &command_tx,
                &sessions,
                config.session_timeout_secs,
            )
            .await;
        }
        RtspMethod::Redirect => {
            send_response(
                &command_tx,
                connection_id,
                cseq,
                405,
                "Method Not Allowed",
                vec![("Allow".to_string(), RTSP_PUBLIC_METHODS.to_string())],
                Bytes::from_static(b"unsupported method: REDIRECT"),
            )
            .await;
        }
        RtspMethod::Extension(method) => {
            send_response(
                &command_tx,
                connection_id,
                cseq,
                405,
                "Method Not Allowed",
                vec![("Allow".to_string(), RTSP_PUBLIC_METHODS.to_string())],
                Bytes::from(format!("unsupported method: {method}")),
            )
            .await;
        }
    }
}
