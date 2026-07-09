use super::*;

pub(super) fn request_session_token(req: &RtspRequest) -> Option<&str> {
    req.session
        .as_deref()
        .map(parse_session_token)
        .filter(|token| !token.is_empty())
}

pub(super) fn parse_session_token(raw: &str) -> &str {
    raw.split(';').next().unwrap_or_default().trim()
}

pub(super) fn validate_request_session(
    connection_id: RtspConnectionId,
    req: &RtspRequest,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    required: bool,
) -> Result<(), (u16, &'static str, &'static [u8])> {
    let session_header = request_session_token(req);
    let current_session = sessions
        .lock()
        .get(&connection_id)
        .map(|state| state.session_id.clone());

    match (required, session_header, current_session.as_deref()) {
        (true, None, _) => Err((454, "Session Not Found", b"missing Session header")),
        (true, Some(_), None) => Err((454, "Session Not Found", b"session is not established")),
        (true, Some(got), Some(expect)) if got != expect => {
            Err((454, "Session Not Found", b"session id mismatch"))
        }
        (false, Some(_), None) => Err((454, "Session Not Found", b"session is not established")),
        (false, Some(got), Some(expect)) if got != expect => {
            Err((454, "Session Not Found", b"session id mismatch"))
        }
        _ => Ok(()),
    }
}

pub(super) fn validate_record_state(
    mode: Option<SessionMode>,
    has_publish: bool,
    configured_track_count: usize,
) -> Result<(), (u16, &'static str, &'static [u8])> {
    if mode != Some(SessionMode::Publish) {
        return Err((
            455,
            "Method Not Valid in This State",
            b"RECORD requires ANNOUNCE/SETUP",
        ));
    }
    if !has_publish {
        return Err((
            455,
            "Method Not Valid in This State",
            b"missing ANNOUNCE before RECORD",
        ));
    }
    if configured_track_count == 0 {
        return Err((
            455,
            "Method Not Valid in This State",
            b"RECORD requires SETUP",
        ));
    }
    Ok(())
}

pub(super) fn validate_pause_state(
    mode: Option<SessionMode>,
    has_play: bool,
    has_publish: bool,
    record_started: bool,
) -> Result<(), (u16, &'static str, &'static [u8])> {
    match mode {
        Some(SessionMode::Play) => {
            if has_play {
                Ok(())
            } else {
                Err((
                    455,
                    "Method Not Valid in This State",
                    b"PAUSE requires PLAY",
                ))
            }
        }
        Some(SessionMode::Publish) => {
            if !has_publish {
                Err((
                    455,
                    "Method Not Valid in This State",
                    b"missing ANNOUNCE before PAUSE",
                ))
            } else if !record_started {
                Err((
                    455,
                    "Method Not Valid in This State",
                    b"PAUSE requires RECORD",
                ))
            } else {
                Ok(())
            }
        }
        None => Err((
            455,
            "Method Not Valid in This State",
            b"PAUSE requires session",
        )),
    }
}
