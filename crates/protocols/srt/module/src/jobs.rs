#[derive(Clone)]
/// Job-defined mode and stream key override for a peer.
///
/// 为对端指定的任务级模式与流密钥覆盖。
struct ForcedSrtMode {
    mode: SrtStreamMode,
    stream_key: StreamKey,
}

/// Pre-built set of driver connect commands and per-peer runtime metadata.
///
/// 预构建的驱动连接命令集合与每个对端的运行时元数据。
struct SrtJobPlan {
    connects: Vec<SrtDriverCommand>,
    forced_modes: HashMap<SrtPeerId, ForcedSrtMode>,
    jobs: HashMap<SrtPeerId, SrtJobRuntime>,
}

#[derive(Clone)]
/// Retry state and original command for a configured job peer.
///
/// 配置任务对端的重试状态与原始命令。
struct SrtJobRuntime {
    command: SrtDriverCommand,
    forced_mode: ForcedSrtMode,
    retry_backoff_ms: u64,
    max_retry_backoff_ms: u64,
    retry_attempt: u32,
}
/// Exponential backoff capped at `max_ms`.
///
/// 上限为 `max_ms` 的指数退避。
fn retry_delay_ms(base_ms: u64, max_ms: u64, attempt: u32) -> u64 {
    let base_ms = base_ms.max(1);
    let max_ms = max_ms.max(base_ms);
    let shift = attempt.min(32);
    let multiplier = 1_u64.checked_shl(shift).unwrap_or(u64::MAX);
    base_ms.saturating_mul(multiplier).min(max_ms)
}
/// Build `SrtJobPlan` from ingress, egress, and relay config jobs.
///
/// 从入口、出口与中继配置任务构建 `SrtJobPlan`。
fn build_job_plan(config: &SrtModuleConfig) -> Result<SrtJobPlan, SdkError> {
    let mut next_peer_id = 1_000_000_u64;
    let connects_capacity =
        config.ingress_jobs.len() + config.egress_jobs.len() + config.relay_jobs.len() * 2;
    let mut connects = Vec::with_capacity(connects_capacity);
    let mut forced_modes = HashMap::with_capacity(connects_capacity);
    let mut jobs = HashMap::with_capacity(connects_capacity);

    for job in &config.ingress_jobs {
        if !job.enabled {
            continue;
        }
        let peer_id = SrtPeerId(next_peer_id);
        next_peer_id += 1;
        let (remote, stream_id, options) = caller_connect_parts(
            &job.source_url,
            SrtStreamMode::Request,
            job.target_stream_key.clone(),
            config,
        )?;
        let command = SrtDriverCommand::ConnectCaller {
            peer_id,
            remote,
            stream_id,
            options,
        };
        let forced_mode = ForcedSrtMode {
            mode: SrtStreamMode::Publish,
            stream_key: stream_key_from_string(&job.target_stream_key),
        };
        connects.push(command.clone());
        forced_modes.insert(peer_id, forced_mode.clone());
        jobs.insert(
            peer_id,
            SrtJobRuntime {
                command,
                forced_mode,
                retry_backoff_ms: job.retry_backoff_ms,
                max_retry_backoff_ms: job.max_retry_backoff_ms,
                retry_attempt: 0,
            },
        );
    }

    for job in &config.egress_jobs {
        if !job.enabled {
            continue;
        }
        let peer_id = SrtPeerId(next_peer_id);
        next_peer_id += 1;
        let (remote, stream_id, options) = caller_connect_parts(
            &job.target_url,
            SrtStreamMode::Publish,
            job.source_stream_key.clone(),
            config,
        )?;
        let command = SrtDriverCommand::ConnectCaller {
            peer_id,
            remote,
            stream_id,
            options,
        };
        let forced_mode = ForcedSrtMode {
            mode: SrtStreamMode::Play,
            stream_key: stream_key_from_string(&job.source_stream_key),
        };
        connects.push(command.clone());
        forced_modes.insert(peer_id, forced_mode.clone());
        jobs.insert(
            peer_id,
            SrtJobRuntime {
                command,
                forced_mode,
                retry_backoff_ms: job.retry_backoff_ms,
                max_retry_backoff_ms: job.max_retry_backoff_ms,
                retry_attempt: 0,
            },
        );
    }

    for job in &config.relay_jobs {
        if !job.enabled {
            continue;
        }
        let relay_stream_key = if job.stream_key.is_empty() {
            format!("relay/{}", job.name)
        } else {
            job.stream_key.clone()
        };

        let ingress_peer_id = SrtPeerId(next_peer_id);
        next_peer_id += 1;
        let (remote, stream_id, options) = caller_connect_parts(
            &job.source_url,
            SrtStreamMode::Request,
            relay_stream_key.clone(),
            config,
        )?;
        let ingress_command = SrtDriverCommand::ConnectCaller {
            peer_id: ingress_peer_id,
            remote,
            stream_id,
            options,
        };
        let ingress_forced_mode = ForcedSrtMode {
            mode: SrtStreamMode::Publish,
            stream_key: stream_key_from_string(&relay_stream_key),
        };
        connects.push(ingress_command.clone());
        forced_modes.insert(ingress_peer_id, ingress_forced_mode.clone());
        jobs.insert(
            ingress_peer_id,
            SrtJobRuntime {
                command: ingress_command,
                forced_mode: ingress_forced_mode,
                retry_backoff_ms: job.retry_backoff_ms,
                max_retry_backoff_ms: job.max_retry_backoff_ms,
                retry_attempt: 0,
            },
        );

        let egress_peer_id = SrtPeerId(next_peer_id);
        next_peer_id += 1;
        let (remote, stream_id, options) = caller_connect_parts(
            &job.target_url,
            SrtStreamMode::Publish,
            relay_stream_key.clone(),
            config,
        )?;
        let egress_command = SrtDriverCommand::ConnectCaller {
            peer_id: egress_peer_id,
            remote,
            stream_id,
            options,
        };
        let egress_forced_mode = ForcedSrtMode {
            mode: SrtStreamMode::Play,
            stream_key: stream_key_from_string(&relay_stream_key),
        };
        connects.push(egress_command.clone());
        forced_modes.insert(egress_peer_id, egress_forced_mode.clone());
        jobs.insert(
            egress_peer_id,
            SrtJobRuntime {
                command: egress_command,
                forced_mode: egress_forced_mode,
                retry_backoff_ms: job.retry_backoff_ms,
                max_retry_backoff_ms: job.max_retry_backoff_ms,
                retry_attempt: 0,
            },
        );
    }

    Ok(SrtJobPlan {
        connects,
        forced_modes,
        jobs,
    })
}

/// Parse an SRT caller URL and build `SrtSessionOptions` for a job.
///
/// 解析 SRT caller URL 并为任务构建 `SrtSessionOptions`。
fn caller_connect_parts(
    url: &str,
    default_mode: SrtStreamMode,
    stream_key: String,
    config: &SrtModuleConfig,
) -> Result<(SocketAddr, Option<String>, SrtSessionOptions), SdkError> {
    let parsed = parse_srt_url(url).map_err(|err| SdkError::InvalidArgument(err.to_string()))?;
    if parsed.mode.is_some_and(|mode| mode != SrtRole::Caller) {
        return Err(SdkError::InvalidArgument(format!(
            "SRT job URL must use mode=caller: {url}"
        )));
    }
    let host = parsed.host.as_deref().unwrap_or("127.0.0.1");
    let remote = (host, parsed.port)
        .to_socket_addrs()
        .map_err(|err| SdkError::InvalidArgument(format!("resolve {host}:{}: {err}", parsed.port)))?
        .next()
        .ok_or_else(|| SdkError::InvalidArgument(format!("no address resolved for {url}")))?;
    if parsed.passphrase.as_deref().is_some_and(str::is_empty) {
        return Err(SdkError::InvalidArgument(
            "SRT URL passphrase must not be empty".to_string(),
        ));
    }
    let encryption = SrtEncryptionOptions {
        enabled: parsed.passphrase.is_some() || config.encryption.enabled,
        passphrase: parsed
            .passphrase
            .clone()
            .unwrap_or_else(|| config.encryption.passphrase.clone()),
        key_length: parsed
            .key_length
            .unwrap_or(match config.encryption.key_length {
                32 => SrtKeyLength::Aes256,
                _ => SrtKeyLength::Aes128,
            }),
    };
    let stream_id = merge_url_token_into_stream_id(parsed.stream_id, parsed.extras.get("token"))?;
    Ok((
        remote,
        stream_id,
        SrtSessionOptions {
            role: SrtRole::Caller,
            mode: default_mode,
            stream_key,
            latency_ms: parsed.latency_ms.unwrap_or(config.latency_ms),
            payload: SrtPayloadKind::MpegTs,
            encryption,
        },
    ))
}

/// Inject an URL `token` query into an access-control stream id.
///
/// 将 URL `token` 查询参数注入访问控制流 id。
fn merge_url_token_into_stream_id(
    stream_id: Option<String>,
    url_token: Option<&String>,
) -> Result<Option<String>, SdkError> {
    if let Some(stream_id) = stream_id
        .as_deref()
        .filter(|value| value.starts_with("#!::"))
    {
        parse_srt_stream_id(stream_id)
            .map_err(|err| SdkError::InvalidArgument(format!("invalid SRT streamid: {err}")))?;
    }
    let Some(token) = url_token.filter(|value| !value.is_empty()) else {
        return Ok(stream_id);
    };
    let token = percent_encode_stream_id_field(token);
    Ok(match stream_id {
        Some(stream_id) if stream_id.starts_with("#!::") => {
            if parse_srt_stream_id(&stream_id)
                .ok()
                .is_some_and(|parsed| parsed.auth_params.contains_key("token"))
            {
                Some(stream_id)
            } else {
                Some(format!("{stream_id},token={token}"))
            }
        }
        Some(stream_id) => {
            let stream_id = percent_encode_stream_id_field(&stream_id);
            Some(format!("#!::r={stream_id},token={token}"))
        }
        None => None,
    })
}

/// Percent-encode a value so it is safe inside an access-control stream id field.
///
/// 对值进行百分号编码，使其在访问控制流 id 字段中安全。
fn percent_encode_stream_id_field(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(&mut out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Parse a `namespace/path` or bare stream key into a `StreamKey`.
///
/// 将 `namespace/path` 或裸流密钥解析为 `StreamKey`。
fn stream_key_from_string(value: &str) -> StreamKey {
    match value.split_once('/') {
        Some((namespace, path)) if !namespace.is_empty() && !path.is_empty() => {
            StreamKey::new(namespace, path)
        }
        _ => StreamKey::new("live", value),
    }
}
/// Schedule a retry for a configured job after a disconnect or error.
///
/// 在断开或错误后为配置任务安排重试。
fn schedule_job_retry(
    ctx: &EngineContext,
    driver: &SrtDriverHandle,
    worker_state: &mut SrtEventWorkerState,
    peer_id: SrtPeerId,
    cancel: CancellationToken,
) {
    let Some(job) = worker_state.jobs.get_mut(&peer_id) else {
        return;
    };
    let delay_ms = retry_delay_ms(
        job.retry_backoff_ms,
        job.max_retry_backoff_ms,
        job.retry_attempt,
    );
    job.retry_attempt = job.retry_attempt.saturating_add(1);
    worker_state
        .forced_modes
        .insert(peer_id, job.forced_mode.clone());

    let command = job.command.clone();
    let driver = driver.clone();
    let runtime = ctx.runtime_api.clone();
    let deadline = MonoTime::from_micros(
        runtime
            .now()
            .as_micros()
            .saturating_add(delay_ms.saturating_mul(1_000)),
    );
    let mut timer = runtime.sleep_until(deadline);
    runtime.spawn(Box::pin(async move {
        let cancel_fut = cancel.cancelled().fuse();
        let sleep_fut = timer.wait().fuse();
        pin_mut!(cancel_fut, sleep_fut);
        select_biased! {
            _ = cancel_fut => {}
            _ = sleep_fut => {
                driver.send(command).await;
            }
        }
    }));
}
