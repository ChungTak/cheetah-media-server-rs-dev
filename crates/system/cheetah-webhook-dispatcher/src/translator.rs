use cheetah_media_api::event::*;
use cheetah_media_api::model::{OnlineState, SessionKind};
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
                payload: stream_payload(
                    &e.header,
                    &e.protocol,
                    e.remote_endpoint.as_deref(),
                    &e.session_id.0,
                    true,
                ),
            }],
            MediaEvent::StreamUnpublished(e) => vec![WebhookDispatch {
                hook_name: "on_stream_changed".to_string(),
                payload: {
                    let mut p = stream_payload(&e.header, "", None, &e.session_id.0, false);
                    p["regist"] = json!(false);
                    p
                },
            }],
            MediaEvent::StreamOnlineChanged(e) => vec![WebhookDispatch {
                hook_name: "on_stream_changed".to_string(),
                payload: {
                    let mut p = media_key_payload(&e.header);
                    p["regist"] = json!(e.online == OnlineState::Online);
                    p
                },
            }],
            MediaEvent::SessionOpened(e) => vec![WebhookDispatch {
                hook_name: session_hook(e.kind),
                payload: session_payload(
                    &e.header,
                    e.kind,
                    &e.protocol,
                    &e.remote_endpoint,
                    &e.session_id.0,
                ),
            }],
            MediaEvent::SessionClosed(e) => vec![WebhookDispatch {
                hook_name: session_hook(e.kind),
                payload: session_payload(&e.header, e.kind, "", &None::<String>, &e.session_id.0),
            }],
            MediaEvent::RecordStarted(e) => vec![WebhookDispatch {
                hook_name: "on_record_progress".to_string(),
                payload: record_payload(&e.header, &e.task_id.0, &e.format, ""),
            }],
            MediaEvent::RecordCompleted(e) => vec![WebhookDispatch {
                hook_name: record_hook(&e.format),
                payload: {
                    let mut p = record_payload(&e.header, &e.task_id.0, &e.format, &e.file_path);
                    p["file_size"] = json!(e.file_size);
                    p["time_len"] = json!(e.time_len_ms / 1000);
                    p["folder"] = json!(&e.folder);
                    p["url"] = json!(e.url.as_deref().unwrap_or(""));
                    p
                },
            }],
            MediaEvent::RtpSessionTimeout(e) => vec![WebhookDispatch {
                hook_name: "on_rtp_server_timeout".to_string(),
                payload: {
                    let mut p = media_key_payload(&e.header);
                    p["local_port"] = json!(e.local_port);
                    p["tcp_mode"] = json!(e.tcp_mode.map(|m| format!("{m:?}")).unwrap_or_default());
                    p["re_use_port"] = json!(e.reuse_port);
                    p["ssrc"] = json!(e.ssrc);
                    p
                },
            }],
            MediaEvent::ServerLifecycle(e) => vec![WebhookDispatch {
                hook_name: server_hook(e.kind),
                payload: {
                    let mut p = json!({
                        "server_id": &e.server_id,
                        "version": &e.version,
                        "status": &e.status,
                    });
                    if let Some(ref mk) = e.header.media_key {
                        merge_media_key(&mut p, mk);
                    }
                    p
                },
            }],
            _ => vec![],
        }
    }
}

fn session_hook(kind: SessionKind) -> String {
    match kind {
        SessionKind::Player => "on_play".to_string(),
        _ => "on_publish".to_string(),
    }
}

fn record_hook(format: &str) -> String {
    if format == "ts" || format == "hls" {
        "on_record_ts".to_string()
    } else {
        "on_record_mp4".to_string()
    }
}

fn server_hook(kind: ServerLifecycleKind) -> String {
    match kind {
        ServerLifecycleKind::Started => "on_server_started".to_string(),
        ServerLifecycleKind::Exited => "on_server_exited".to_string(),
        ServerLifecycleKind::Keepalive => "on_server_keepalive".to_string(),
    }
}

fn stream_payload(
    header: &EventHeader,
    protocol: &str,
    remote_endpoint: Option<&str>,
    session_id: &str,
    regist: bool,
) -> Value {
    let mut p = media_key_payload(header);
    p["protocol"] = json!(protocol);
    p["session_id"] = json!(session_id);
    if let Some(ep) = remote_endpoint {
        if let Some(addr) = parse_endpoint_addr(ep) {
            p["ip"] = json!(addr.0);
            p["port"] = json!(addr.1);
        }
    }
    p["regist"] = json!(regist);
    p
}

fn session_payload(
    header: &EventHeader,
    kind: SessionKind,
    protocol: &str,
    remote_endpoint: &Option<String>,
    session_id: &str,
) -> Value {
    let mut p = media_key_payload(header);
    p["kind"] = json!(format!("{kind:?}"));
    p["protocol"] = json!(protocol);
    p["session_id"] = json!(session_id);
    if let Some(ep) = remote_endpoint {
        if let Some(addr) = parse_endpoint_addr(ep) {
            p["ip"] = json!(addr.0);
            p["port"] = json!(addr.1);
        }
    }
    p
}

fn record_payload(header: &EventHeader, task_id: &str, format: &str, file_path: &str) -> Value {
    let mut p = media_key_payload(header);
    p["task_id"] = json!(task_id);
    p["format"] = json!(format);
    p["file_path"] = json!(file_path);
    p
}

fn media_key_payload(header: &EventHeader) -> Value {
    let mut p = json!({
        "event_id": &header.event_id,
        "occurred_at": header.occurred_at,
    });
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

fn parse_endpoint_addr(ep: &str) -> Option<(String, u16)> {
    ep.rsplit_once(':')
        .and_then(|(ip, port)| port.parse::<u16>().ok().map(|p| (ip.to_string(), p)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_media_api::ids::{AppName, MediaKey, StreamName, VhostName};

    fn sample_header() -> EventHeader {
        EventHeader {
            event_id: "evt-1".to_string(),
            occurred_at: 1,
            sequence: None,
            media_key: Some(MediaKey {
                vhost: VhostName("__defaultVhost__".to_string()),
                app: AppName("live".to_string()),
                stream: StreamName("test".to_string()),
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
            stream: StreamName("test".to_string()),
            schema: None,
        }
    }

    #[test]
    fn translates_stream_published_to_on_publish() {
        let event = MediaEvent::StreamPublished(StreamPublished {
            header: sample_header(),
            protocol: "rtmp".to_string(),
            remote_endpoint: Some("192.168.1.2:1935".to_string()),
            session_id: cheetah_media_api::ids::SessionId("s1".to_string()),
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        let d = &dispatches[0];
        assert_eq!(d.hook_name, "on_publish");
        assert_eq!(d.payload["app"], "live");
        assert_eq!(d.payload["stream"], "test");
        assert_eq!(d.payload["protocol"], "rtmp");
        assert_eq!(d.payload["ip"], "192.168.1.2");
        assert_eq!(d.payload["port"], 1935);
        assert_eq!(d.payload["regist"], true);
    }

    #[test]
    fn translates_session_opened_to_on_play() {
        let event = MediaEvent::SessionOpened(SessionOpened {
            header: sample_header(),
            kind: SessionKind::Player,
            protocol: "rtsp".to_string(),
            remote_endpoint: Some("10.0.0.5:554".to_string()),
            session_id: cheetah_media_api::ids::SessionId("s2".to_string()),
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].hook_name, "on_play");
        assert_eq!(dispatches[0].payload["kind"], "Player");
    }

    #[test]
    fn translates_record_completed_to_on_record_mp4() {
        let mut header = sample_header();
        header.media_key = Some(sample_media_key());
        let event = MediaEvent::RecordCompleted(RecordCompleted {
            header,
            task_id: cheetah_media_api::ids::RecordTaskId("task-1".to_string()),
            format: "mp4".to_string(),
            file_path: "/tmp/1.mp4".to_string(),
            file_size: 1024,
            time_len_ms: 15000,
            folder: "/tmp".to_string(),
            url: Some("http://x/1.mp4".to_string()),
        });
        let dispatches = ZlmWebhookTranslator.translate(&event);
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].hook_name, "on_record_mp4");
        assert_eq!(dispatches[0].payload["file_size"], 1024);
        assert_eq!(dispatches[0].payload["time_len"], 15);
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
