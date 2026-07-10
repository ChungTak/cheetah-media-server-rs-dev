use super::*;
/// `send_basic_ok_with_session` function.
/// `send_basic_ok_with_session` 函数.
pub(super) async fn send_basic_ok_with_session(
    connection_id: RtspConnectionId,
    cseq: Option<u32>,
    command_tx: &RtspCoreCommandSender,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    session_timeout_secs: u32,
) {
    let session_id = sessions
        .lock()
        .get(&connection_id)
        .map(|state| state.session_id.clone());
    let mut headers = Vec::new();
    if let Some(session_id) = session_id {
        headers.push((
            "Session".to_string(),
            session_header_value(&session_id, session_timeout_secs),
        ));
    }
    send_response(
        command_tx,
        connection_id,
        cseq,
        200,
        "OK",
        headers,
        Bytes::new(),
    )
    .await;
}

/// `handle_get_parameter` function.
/// `handle_get_parameter` 函数.
pub(super) async fn handle_get_parameter(
    connection_id: RtspConnectionId,
    req: RtspRequest,
    command_tx: &RtspCoreCommandSender,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    session_timeout_secs: u32,
) {
    let session_id = sessions
        .lock()
        .get(&connection_id)
        .map(|state| state.session_id.clone());
    let request_content_type = header_value(&req, "Content-Type").map(str::to_string);
    let (headers, body) = build_get_parameter_response(
        session_id,
        request_content_type.as_deref(),
        req.body,
        session_timeout_secs,
    );

    send_response(
        command_tx,
        connection_id,
        req.cseq,
        200,
        "OK",
        headers,
        body,
    )
    .await;
}

/// `send_response` function.
/// `send_response` 函数.
pub(super) async fn send_response(
    command_tx: &RtspCoreCommandSender,
    connection_id: RtspConnectionId,
    cseq: Option<u32>,
    status_code: u16,
    reason: &str,
    headers: Vec<(String, String)>,
    body: Bytes,
) {
    let result = command_tx
        .send_core(
            connection_id,
            RtspCommand::SendResponse {
                cseq,
                status_code,
                reason: reason.to_string(),
                headers,
                body,
            },
        )
        .await;
    if result.is_err() {
        warn!(
            connection_id,
            status_code, "send_response failed: channel closed"
        );
    }
}

/// Builds `rtp_info_header` output.
/// 构建 `rtp_info_header` 输出.
pub(super) fn build_rtp_info_header(
    base_uri: Option<&str>,
    control_to_track: &HashMap<String, TrackId>,
    play_tracks: &HashMap<TrackId, PlayTrackState>,
) -> Option<String> {
    if play_tracks.is_empty() {
        return None;
    }

    let mut control_by_track = HashMap::<TrackId, String>::new();
    for (control, track_id) in control_to_track {
        control_by_track
            .entry(*track_id)
            .or_insert_with(|| control.clone());
    }

    let mut entries: Vec<(u16, String)> = Vec::new();
    for (track_id, state) in play_tracks {
        let control = control_by_track
            .get(track_id)
            .cloned()
            .unwrap_or_else(|| format!("trackID={}", track_id.0));
        let url = if let Some(base_uri) = base_uri {
            format!("{}/{}", base_uri.trim_end_matches('/'), control)
        } else {
            control
        };
        let sort_key = match &state.transport {
            PlayTransport::TcpInterleaved { rtp_channel, .. } => u16::from(*rtp_channel),
            PlayTransport::UdpUnicast { target_rtp, .. } => {
                1000u16.saturating_add(target_rtp.port())
            }
            PlayTransport::UdpMulticast { target_rtp, .. } => {
                1000u16.saturating_add(target_rtp.port())
            }
        };
        entries.push((
            sort_key,
            format!(
                "url={url};seq={};rtptime={}",
                state.seq, state.last_rtp_timestamp
            ),
        ));
    }
    entries.sort_by_key(|(channel, _)| *channel);
    Some(
        entries
            .into_iter()
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// Builds `play_response_headers` output.
/// 构建 `play_response_headers` 输出.
pub(super) fn build_play_response_headers(
    session_id: String,
    response_range: String,
    rtp_info: Option<String>,
    session_timeout_secs: u32,
) -> Vec<(String, String)> {
    let mut headers = vec![
        (
            "Session".to_string(),
            session_header_value(&session_id, session_timeout_secs),
        ),
        ("Range".to_string(), response_range),
        ("Scale".to_string(), "1.0".to_string()),
    ];
    if let Some(rtp_info) = rtp_info {
        headers.push(("RTP-Info".to_string(), rtp_info));
    }
    headers
}

/// Builds `pause_response_headers` output.
/// 构建 `pause_response_headers` 输出.
pub(super) fn build_pause_response_headers(
    session_id: String,
    response_range: Option<String>,
    session_timeout_secs: u32,
) -> Vec<(String, String)> {
    let mut headers = vec![(
        "Session".to_string(),
        session_header_value(&session_id, session_timeout_secs),
    )];
    if let Some(range) = response_range {
        headers.push(("Range".to_string(), range));
        headers.push(("Scale".to_string(), "1.0".to_string()));
    }
    headers
}

/// Builds `get_parameter_response` output.
/// 构建 `get_parameter_response` 输出.
pub(super) fn build_get_parameter_response(
    session_id: Option<String>,
    request_content_type: Option<&str>,
    request_body: Bytes,
    session_timeout_secs: u32,
) -> (Vec<(String, String)>, Bytes) {
    let mut headers = Vec::new();
    if let Some(session_id) = session_id {
        headers.push((
            "Session".to_string(),
            session_header_value(&session_id, session_timeout_secs),
        ));
    }

    if !request_body.is_empty() {
        let content_type = request_content_type
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("text/parameters");
        headers.push(("Content-Type".to_string(), content_type.to_string()));
    }

    (headers, request_body)
}

/// `session_header_value` function.
/// `session_header_value` 函数.
pub(super) fn session_header_value(session_id: &str, session_timeout_secs: u32) -> String {
    format!("{session_id};timeout={session_timeout_secs}")
}

/// `resolve_play_response_range` function.
/// `resolve_play_response_range` 函数.
pub(super) fn resolve_play_response_range(requested_range: Option<String>) -> String {
    requested_range.unwrap_or_else(|| "npt=0.000-".to_string())
}

/// `resolve_pause_response_range` function.
/// `resolve_pause_response_range` 函数.
pub(super) fn resolve_pause_response_range(
    requested_range: Option<String>,
    last_play_range: Option<&str>,
) -> String {
    if let Some(requested_range) = requested_range {
        requested_range
    } else if let Some(last_play_range) = last_play_range {
        last_play_range.to_string()
    } else {
        "npt=0.000-".to_string()
    }
}

/// `apply_play_response_range` function.
/// `apply_play_response_range` 函数.
pub(super) fn apply_play_response_range(
    state: &mut RtspConnectionState,
    requested_range: Option<String>,
) -> String {
    let response_range = resolve_play_response_range(requested_range);
    state.play_response_range = Some(response_range.clone());
    response_range
}

/// `apply_pause_response_range` function.
/// `apply_pause_response_range` 函数.
pub(super) fn apply_pause_response_range(
    state: &mut RtspConnectionState,
    requested_range: Option<String>,
) -> String {
    let response_range =
        resolve_pause_response_range(requested_range, state.play_response_range.as_deref());
    state.play_response_range = Some(response_range.clone());
    response_range
}
