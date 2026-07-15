use std::path::Path;

use cheetah_media_api::event::*;
use cheetah_media_api::model::{OnlineState, RtpTcpMode, SessionKind};
use serde_json::{json, Value};

/// One concrete webhook dispatch derived from a `MediaEvent`.
///
/// 从一个 `MediaEvent` 派生出的具体 webhook 投递。
#[derive(Debug, Clone)]
pub struct WebhookDispatch {
    pub hook_name: String,
    pub payload: Value,
}

/// Translates a `MediaEvent` into one or more webhook dispatches.
///
/// 将 `MediaEvent` 翻译成一个或多个 webhook 投递。
pub trait WebhookTranslator: Send + Sync {
    fn translate(&self, event: &MediaEvent) -> Vec<WebhookDispatch>;
}

/// ZLMediaKit-compatible translator.
///
/// ZLMediaKit 兼容翻译器。
#[derive(Debug, Clone, Default)]
pub struct ZlmWebhookTranslator;

impl WebhookTranslator for ZlmWebhookTranslator {
    fn translate(&self, event: &MediaEvent) -> Vec<WebhookDispatch> {
        match event {
            MediaEvent::StreamPublished(e) => vec![WebhookDispatch {
                hook_name: "on_publish".to_string(),
                payload: publish_play_payload(
                    &e.header,
                    &e.protocol,
                    e.remote_endpoint.as_deref(),
                    &e.session_id.0,
                ),
            }],
            MediaEvent::StreamUnpublished(e) => vec![WebhookDispatch {
                hook_name: "on_stream_changed".to_string(),
                payload: {
                    let mut p = media_key_payload(&e.header);
                    p["regist"] = json!(false);
                    p
                },
            }],
            MediaEvent::StreamOnlineChanged(e) => vec![WebhookDispatch {
                hook_name: "on_stream_changed".to_string(),
                payload: {
                    let mut p = media_key_payload(&e.header);
                    if let Some(schema) = e.schema {
                        p["schema"] = json!(schema.to_string());
                    }
                    p["regist"] = json!(e.online == OnlineState::Online);
                    p
                },
            }],
            MediaEvent::SessionOpened(e) => vec![WebhookDispatch {
                hook_name: session_hook(e.kind),
                payload: publish_play_payload(
                    &e.header,
                    &e.protocol,
                    e.remote_endpoint.as_deref(),
                    &e.session_id.0,
                ),
            }],
            // `SessionClosed` is a per-session event; it does not automatically mean that the
            // stream has no remaining readers, so we do not synthesize `on_stream_none_reader`.
            MediaEvent::SessionClosed(_) => vec![],
            MediaEvent::RecordStarted(_) => vec![],
            MediaEvent::RecordProgress(e) => {
                let Some(ref path) = e.file_path else {
                    return vec![];
                };
                vec![WebhookDispatch {
                    hook_name: "on_record_ts".to_string(),
                    payload: record_info_payload(
                        &e.header,
                        path,
                        e.size_bytes,
                        e.duration_ms,
                        None,
                    ),
                }]
            }
            MediaEvent::RecordCompleted(e) => {
                let hook = if e.format == "ts" || e.format == "hls" {
                    "on_record_ts"
                } else {
                    "on_record_mp4"
                };
                vec![WebhookDispatch {
                    hook_name: hook.to_string(),
                    payload: record_info_payload(
                        &e.header,
                        &e.file_path,
                        e.file_size,
                        e.time_len_ms,
                        e.url.as_deref(),
                    ),
                }]
            }
            MediaEvent::SnapshotCompleted(_) => vec![],
            MediaEvent::RtpSessionTimeout(e) => vec![WebhookDispatch {
                hook_name: "on_rtp_server_timeout".to_string(),
                payload: rtp_timeout_payload(e),
            }],
            MediaEvent::ProxyStateChanged(_) => vec![],
            MediaEvent::ServerLifecycle(e) => vec![WebhookDispatch {
                hook_name: server_hook(e.kind),
                payload: json!({
                    "server_id": &e.server_id,
                    "version": &e.version,
                    "status": &e.status,
                }),
            }],
        }
    }
}

fn session_hook(kind: SessionKind) -> String {
    match kind {
        SessionKind::Player => "on_play".to_string(),
        _ => "on_publish".to_string(),
    }
}

fn server_hook(kind: ServerLifecycleKind) -> String {
    match kind {
        ServerLifecycleKind::Started => "on_server_started".to_string(),
        ServerLifecycleKind::Exited => "on_server_exited".to_string(),
        ServerLifecycleKind::Keepalive => "on_server_keepalive".to_string(),
    }
}

fn media_key_payload(header: &EventHeader) -> Value {
    let mut p = json!({});
    if let Some(ref mk) = header.media_key {
        merge_media_key(&mut p, mk);
    }
    p
}

fn merge_media_key(p: &mut Value, mk: &cheetah_media_api::ids::MediaKey) {
    p["vhost"] = json!(&mk.vhost.0);
    p["app"] = json!(&mk.app.0);
    p["stream"] = json!(&mk.stream.0);
    if let Some(schema) = mk.schema {
        p["schema"] = json!(schema.to_string());
    }
}

/// Payload for `on_publish` and `on_play`.
///
/// Includes the ZLMediaKit fields: app, stream, vhost, schema, protocol, id,
/// ip, port and params.
fn publish_play_payload(
    header: &EventHeader,
    protocol: &str,
    remote_endpoint: Option<&str>,
    session_id: &str,
) -> Value {
    let mut p = media_key_payload(header);
    p["schema"] = json!(protocol);
    p["protocol"] = json!(protocol);
    p["id"] = json!(session_id);
    p["params"] = json!("");
    if let Some(ep) = remote_endpoint {
        if let Some((ip, port)) = parse_endpoint_addr(ep) {
            p["ip"] = json!(ip);
            p["port"] = json!(port);
        }
    }
    p
}

fn record_info_payload(
    header: &EventHeader,
    file_path: &str,
    file_size: u64,
    duration_ms: u64,
    url: Option<&str>,
) -> Value {
    let mut p = media_key_payload(header);
    let path = Path::new(file_path);
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let folder = path
        .parent()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let occurred_at = header.occurred_at.max(0) as u64;
    let start_time_ms = occurred_at.saturating_sub(duration_ms);
    p["file_name"] = json!(file_name);
    p["file_path"] = json!(file_path);
    p["file_size"] = json!(file_size);
    p["folder"] = json!(folder);
    p["start_time"] = json!(start_time_ms / 1000);
    p["time_len"] = json!(duration_ms as f64 / 1000.0);
    p["url"] = json!(url.unwrap_or(""));
    p
}

fn rtp_timeout_payload(e: &RtpSessionTimeout) -> Value {
    let mut p = json!({});
    if let Some(ref mk) = e.header.media_key {
        p["stream_id"] = json!(&mk.stream.0);
    } else {
        p["stream_id"] = json!(&e.session_id.0);
    }
    p["local_port"] = json!(e.local_port.unwrap_or(0));
    p["tcp_mode"] = json!(tcp_mode_int(e.tcp_mode));
    p["re_use_port"] = json!(e.reuse_port);
    p["ssrc"] = json!(e.ssrc.unwrap_or(0));
    p
}

fn tcp_mode_int(mode: Option<RtpTcpMode>) -> i64 {
    match mode {
        None => 0,
        Some(RtpTcpMode::Passive) => 1,
        Some(RtpTcpMode::Active) => 2,
    }
}

fn parse_endpoint_addr(ep: &str) -> Option<(String, u16)> {
    ep.rsplit_once(':')
        .and_then(|(ip, port)| port.parse::<u16>().ok().map(|p| (ip.to_string(), p)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_media_api::ids::{
        AppName, MediaKey, RecordTaskId, RtpSessionId, SessionId, StreamName, VhostName,
    };
    use cheetah_media_api::model::CloseReason;
    use cheetah_media_api::MediaSchema;

    fn sample_header() -> EventHeader {
        EventHeader {
            event_id: "evt-1".to_string(),
            occurred_at: 1_600_000_000_000,
            sequence: None,
            media_key: Some(MediaKey {
                vhost: VhostName("__defaultVhost__".to_string()),
                app: AppName("live".to_string()),
                stream: StreamName("obs".to_string()),
                schema: None,
            }),
            source: "test".to_string(),
            correlation_id: None,
        }
    }

    fn sample_media_key() -> MediaKey {
        MediaKey {
            vhost: VhostName("__defaultVhost__".to_string()),
            app: AppName("live".to_string()),
            stream: StreamName("obs".to_string()),
            schema: None,
        }
    }

    fn header_with_schema() -> EventHeader {
        let mut h = sample_header();
        h.media_key = Some(MediaKey {
            vhost: VhostName("__defaultVhost__".to_string()),
            app: AppName("live".to_string()),
            stream: StreamName("obs".to_string()),
            schema: Some(MediaSchema::Rtmp),
        });
        h
    }

    #[test]
    fn translates_stream_published_to_on_publish() {
        let event = MediaEvent::StreamPublished(StreamPublished {
            header: sample_header(),
            protocol: "rtmp".to_string(),
            remote_endpoint: Some("192.168.1.2:1935".to_string()),
            session_id: SessionId("s1".to_string()),
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        let d = &dispatches[0];
        assert_eq!(d.hook_name, "on_publish");
        assert_eq!(d.payload["app"], "live");
        assert_eq!(d.payload["stream"], "obs");
        assert_eq!(d.payload["vhost"], "__defaultVhost__");
        assert_eq!(d.payload["schema"], "rtmp");
        assert_eq!(d.payload["protocol"], "rtmp");
        assert_eq!(d.payload["id"], "s1");
        assert_eq!(d.payload["ip"], "192.168.1.2");
        assert_eq!(d.payload["port"], 1935);
        assert_eq!(d.payload["params"], "");
    }

    #[test]
    fn translates_session_opened_player_to_on_play() {
        let event = MediaEvent::SessionOpened(SessionOpened {
            header: sample_header(),
            kind: SessionKind::Player,
            protocol: "rtsp".to_string(),
            remote_endpoint: Some("10.0.0.5:554".to_string()),
            session_id: SessionId("s2".to_string()),
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].hook_name, "on_play");
        assert_eq!(dispatches[0].payload["app"], "live");
        assert_eq!(dispatches[0].payload["id"], "s2");
        assert_eq!(dispatches[0].payload["protocol"], "rtsp");
        assert_eq!(dispatches[0].payload["ip"], "10.0.0.5");
        assert_eq!(dispatches[0].payload["port"], 554);
    }

    #[test]
    fn translates_stream_unpublished_to_on_stream_changed_regist_false() {
        let event = MediaEvent::StreamUnpublished(StreamUnpublished {
            header: sample_header(),
            session_id: SessionId("s1".to_string()),
            reason: CloseReason::Normal,
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].hook_name, "on_stream_changed");
        assert_eq!(dispatches[0].payload["app"], "live");
        assert_eq!(dispatches[0].payload["stream"], "obs");
        assert_eq!(dispatches[0].payload["regist"], false);
        assert!(dispatches[0].payload["protocol"].is_null());
    }

    #[test]
    fn translates_stream_online_changed_to_on_stream_changed_regist_true() {
        let event = MediaEvent::StreamOnlineChanged(StreamOnlineChanged {
            header: header_with_schema(),
            online: OnlineState::Online,
            schema: Some(MediaSchema::Rtmp),
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].hook_name, "on_stream_changed");
        assert_eq!(dispatches[0].payload["app"], "live");
        assert_eq!(dispatches[0].payload["stream"], "obs");
        assert_eq!(dispatches[0].payload["schema"], "rtmp");
        assert_eq!(dispatches[0].payload["regist"], true);
    }

    #[test]
    fn translates_record_completed_to_on_record_mp4_golden() {
        let mut header = sample_header();
        header.media_key = Some(sample_media_key());
        let event = MediaEvent::RecordCompleted(RecordCompleted {
            header,
            task_id: RecordTaskId("task-1".to_string()),
            format: "mp4".to_string(),
            file_path: "/record/live/obs/15-53-02.mp4".to_string(),
            file_size: 1_913_597,
            time_len_ms: 11_000,
            folder: "/record/live/obs/".to_string(),
            url: Some("record/live/obs/15-53-02.mp4".to_string()),
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        let d = &dispatches[0];
        assert_eq!(d.hook_name, "on_record_mp4");
        assert_eq!(d.payload["app"], "live");
        assert_eq!(d.payload["stream"], "obs");
        assert_eq!(d.payload["vhost"], "__defaultVhost__");
        assert_eq!(d.payload["file_name"], "15-53-02.mp4");
        assert_eq!(d.payload["file_path"], "/record/live/obs/15-53-02.mp4");
        assert_eq!(d.payload["file_size"], 1_913_597);
        assert_eq!(d.payload["folder"], "/record/live/obs");
        assert_eq!(d.payload["time_len"], 11.0);
        assert_eq!(d.payload["url"], "record/live/obs/15-53-02.mp4");
        assert_eq!(d.payload["start_time"], 1_599_999_989);
    }

    #[test]
    fn translates_record_progress_to_on_record_ts() {
        let mut header = sample_header();
        header.media_key = Some(sample_media_key());
        let event = MediaEvent::RecordProgress(RecordProgress {
            header,
            task_id: RecordTaskId("task-1".to_string()),
            duration_ms: 6_000,
            size_bytes: 1_508_161,
            file_path: Some("/record/live/obs/2019-09-20/15-53-02.ts".to_string()),
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].hook_name, "on_record_ts");
        assert_eq!(dispatches[0].payload["file_name"], "15-53-02.ts");
        assert_eq!(dispatches[0].payload["file_size"], 1_508_161);
        assert_eq!(dispatches[0].payload["time_len"], 6.0);
    }

    #[test]
    fn translates_record_completed_ts_to_on_record_ts() {
        let mut header = sample_header();
        header.media_key = Some(sample_media_key());
        let event = MediaEvent::RecordCompleted(RecordCompleted {
            header,
            task_id: RecordTaskId("task-1".to_string()),
            format: "ts".to_string(),
            file_path: "/record/live/obs/2019-09-20/15-53-02.ts".to_string(),
            file_size: 1_508_161,
            time_len_ms: 6_000,
            folder: "/record/live/obs/".to_string(),
            url: None,
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].hook_name, "on_record_ts");
    }

    #[test]
    fn translates_rtp_session_timeout_to_on_rtp_server_timeout() {
        let event = MediaEvent::RtpSessionTimeout(RtpSessionTimeout {
            header: sample_header(),
            session_id: RtpSessionId("rtp-1".to_string()),
            local_port: Some(30_000),
            tcp_mode: Some(RtpTcpMode::Passive),
            reuse_port: true,
            ssrc: Some(0x12345678),
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].hook_name, "on_rtp_server_timeout");
        assert_eq!(dispatches[0].payload["stream_id"], "obs");
        assert_eq!(dispatches[0].payload["local_port"], 30_000);
        assert_eq!(dispatches[0].payload["tcp_mode"], 1);
        assert_eq!(dispatches[0].payload["re_use_port"], true);
        assert_eq!(dispatches[0].payload["ssrc"], 0x12345678);
    }

    #[test]
    fn translates_server_lifecycle_to_on_server_started() {
        let event = MediaEvent::ServerLifecycle(ServerLifecycle {
            header: sample_header(),
            kind: ServerLifecycleKind::Started,
            server_id: "srv-1".to_string(),
            version: "0.1.0".to_string(),
            status: "ok".to_string(),
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].hook_name, "on_server_started");
        assert_eq!(dispatches[0].payload["server_id"], "srv-1");
    }

    #[test]
    fn record_progress_without_file_path_is_ignored() {
        let mut header = sample_header();
        header.media_key = Some(sample_media_key());
        let event = MediaEvent::RecordProgress(RecordProgress {
            header,
            task_id: RecordTaskId("task-1".to_string()),
            duration_ms: 1_000,
            size_bytes: 100,
            file_path: None,
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert!(dispatches.is_empty());
    }

    #[test]
    fn parse_endpoint_addr_handles_ipv4_with_port() {
        assert_eq!(
            parse_endpoint_addr("192.168.1.1:8080"),
            Some(("192.168.1.1".to_string(), 8080))
        );
    }

    #[test]
    fn parse_endpoint_addr_rejects_invalid() {
        assert_eq!(parse_endpoint_addr("no-port"), None);
    }
}
