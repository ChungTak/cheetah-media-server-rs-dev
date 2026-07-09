pub mod error;
pub mod request;
pub mod session;

pub use error::HttpFlvCoreError;
pub use request::{
    parse_play_request_target, validate_websocket_upgrade, websocket_accept_key, HttpFlvQueryMode,
    HttpFlvTransport, HttpMethod, HttpRequestHead, HttpResponseHead, ParsedPlayRequest,
    StreamKeyParts, WebSocketMessage,
};
pub use session::{
    CloseReason, HttpFlvCore, HttpFlvCoreCommand, HttpFlvCoreInput, HttpFlvCoreOutput, HttpFlvEvent,
};

#[cfg(test)]
mod tests {
    use cheetah_rtmp_core::RtmpFlvPlayMode;

    use super::*;

    fn make_get(target: &str, headers: &[(&str, &str)]) -> HttpRequestHead {
        HttpRequestHead {
            method: HttpMethod::Get,
            method_raw: "GET".to_string(),
            target: target.to_string(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn parses_route_and_query_modes() {
        let parsed = parse_play_request_target("/live/stream.flv?type=enhanced").expect("parse");
        assert_eq!(parsed.stream_key.namespace, "live");
        assert_eq!(parsed.stream_key.stream_path, "stream");
        assert_eq!(parsed.mode, HttpFlvQueryMode::Enhanced);

        let fast = parse_play_request_target("/live/stream.flv?type=fastPts").expect("parse");
        assert_eq!(fast.mode, HttpFlvQueryMode::FastPts);
    }

    #[test]
    fn rejects_invalid_method_and_path() {
        let mut core = HttpFlvCore::new();
        // POST to a valid .flv path now triggers publish
        let outputs = core
            .handle_input(HttpFlvCoreInput::RequestHead(HttpRequestHead {
                method: HttpMethod::Post,
                method_raw: "POST".to_string(),
                target: "/live/stream.flv".to_string(),
                headers: Vec::new(),
            }))
            .expect("handle");
        assert!(outputs
            .iter()
            .any(|out| matches!(out, HttpFlvCoreOutput::SendHttpResponse(head) if head.status_code == 200)));
        assert!(outputs.iter().any(|out| matches!(
            out,
            HttpFlvCoreOutput::Event(HttpFlvEvent::PublishRequested { .. })
        )));

        // Unknown method still returns 405
        let mut core2 = HttpFlvCore::new();
        let outputs2 = core2
            .handle_input(HttpFlvCoreInput::RequestHead(HttpRequestHead {
                method: HttpMethod::Other,
                method_raw: "DELETE".to_string(),
                target: "/live/stream.flv".to_string(),
                headers: Vec::new(),
            }))
            .expect("handle");
        assert!(outputs2
            .iter()
            .any(|out| matches!(out, HttpFlvCoreOutput::SendHttpResponse(head) if head.status_code == 405)));

        assert!(matches!(
            parse_play_request_target("/live/stream.ts"),
            Err(HttpFlvCoreError::InvalidFlvPath { .. })
        ));
    }

    #[test]
    fn handles_http_get_and_options() {
        let mut core = HttpFlvCore::new();
        let get_outputs = core
            .handle_input(HttpFlvCoreInput::RequestHead(make_get(
                "/live/stream.flv",
                &[("Host", "example.com")],
            )))
            .expect("get");
        assert!(get_outputs
            .iter()
            .any(|out| matches!(out, HttpFlvCoreOutput::SendHttpResponse(head) if head.status_code == 200)));
        assert!(get_outputs.iter().any(|out| {
            matches!(
                out,
                HttpFlvCoreOutput::Event(HttpFlvEvent::PlayRequested {
                    play_mode: RtmpFlvPlayMode::Normal,
                    ..
                })
            )
        }));

        let mut options_core = HttpFlvCore::new();
        let options_outputs = options_core
            .handle_input(HttpFlvCoreInput::RequestHead(HttpRequestHead {
                method: HttpMethod::Options,
                method_raw: "OPTIONS".to_string(),
                target: "/live/stream.flv".to_string(),
                headers: Vec::new(),
            }))
            .expect("options");
        assert!(options_outputs
            .iter()
            .any(|out| matches!(out, HttpFlvCoreOutput::SendHttpResponse(head) if head.status_code == 204)));
    }

    #[test]
    fn validates_websocket_upgrade_and_accept_key() {
        let head = make_get(
            "/live/stream.flv?type=enhanced",
            &[
                ("Connection", "Upgrade"),
                ("Upgrade", "websocket"),
                ("Sec-WebSocket-Version", "13"),
                ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
        );
        let accept = validate_websocket_upgrade(&head).expect("accept key");
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");

        let mut core = HttpFlvCore::new();
        let outputs = core
            .handle_input(HttpFlvCoreInput::RequestHead(head))
            .expect("ws upgrade");
        assert!(outputs
            .iter()
            .any(|out| matches!(out, HttpFlvCoreOutput::SendHttpResponse(resp) if resp.status_code == 101)));
        assert!(outputs.iter().any(|out| {
            matches!(
                out,
                HttpFlvCoreOutput::Event(HttpFlvEvent::PlayRequested {
                    transport: HttpFlvTransport::WebSocket,
                    play_mode: RtmpFlvPlayMode::Enhanced,
                    ..
                })
            )
        }));
    }
}
