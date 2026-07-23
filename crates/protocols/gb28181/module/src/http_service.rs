//! HTTP control API for the GB28181 module.
//!
//! This module only normalizes incoming media parameters and forwards them to the
//! typed `RtpSessionApi`; it does not parse or emit SIP/SDP/XML signaling.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::media_api::ids::MediaKey;
use cheetah_sdk::media_api::port::MediaRequestContext;
use cheetah_sdk::media_api::rtp_session::{
    MediaContainer, OpenRtpReceiver, OpenRtpSender, OpenRtpTalk, RtpDirection, RtpPayloadBinding,
    RtpSessionApi, RtpSessionParamsBuilder, RtpSessionRef, RtpTransport, StopRtpSession,
};
use cheetah_sdk::{
    EngineContext, HttpMethod, HttpRequest, HttpResponse, ModuleHttpService, SdkError,
};
use parking_lot::Mutex;

use crate::request::{GbRecvRequest, GbSendRequest, GbStopRequest, GbTalkRequest};

/// HTTP control API for the GB28181 module.
///
/// GB28181 模块的 HTTP 控制 API。
pub(crate) struct GbHttpService {
    engine: EngineContext,
    /// session_key -> (device_id, rtp_session_ref)
    active_sessions: Arc<Mutex<HashMap<String, (String, RtpSessionRef)>>>,
    /// Default local RTP port for media reception when REST request omits `port`.
    default_media_port: u16,
}

/// `GbHttpService` helpers.
///
/// `GbHttpService` 辅助。
impl GbHttpService {
    /// Create a new `GbHttpService`.
    pub(crate) fn new(
        engine: EngineContext,
        active_sessions: Arc<Mutex<HashMap<String, (String, RtpSessionRef)>>>,
        default_media_port: u16,
    ) -> Self {
        Self {
            engine,
            active_sessions,
            default_media_port,
        }
    }

    /// Return the typed RTP session provider.
    fn rtp_session_api(&self) -> Result<Arc<dyn RtpSessionApi>, SdkError> {
        self.engine.media_services.rtp_session().ok_or_else(|| {
            SdkError::Unavailable("RTP session provider is not available".to_string())
        })
    }

    /// Build the default PS payload binding used by GB28181 streams.
    fn ps_payload_binding(&self) -> RtpPayloadBinding {
        RtpPayloadBinding {
            payload_type: 96,
            codec: "PS".to_string(),
            clock_rate: 90000,
            channels: None,
            packet_duration_ms: None,
        }
    }

    /// Construct a `MediaKey` from the GB app/stream aliases.
    fn media_key(&self, app: &str, stream: &str) -> Result<MediaKey, SdkError> {
        MediaKey::with_default_vhost(app, stream, None)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid media key: {e}")))
    }

    /// Open an RTP receiver for the given GB session and return the descriptor.
    async fn open_gb_receiver(
        &self,
        app: &str,
        stream: &str,
        ssrc: u32,
        local_port: u16,
    ) -> Result<cheetah_sdk::media_api::rtp_session::RtpSessionDescriptor, SdkError> {
        let media_key = self.media_key(app, stream)?;
        let local_endpoint_hint = SocketAddr::new(
            "0.0.0.0"
                .parse::<std::net::IpAddr>()
                .map_err(|e| SdkError::Internal(e.to_string()))?,
            local_port,
        );
        let params = RtpSessionParamsBuilder::new(media_key, RtpDirection::Receive)
            .transport(RtpTransport::Udp)
            .container(MediaContainer::Ps)
            .ssrc(ssrc)
            .payload_binding(self.ps_payload_binding())
            .local_endpoint_hint(local_endpoint_hint)
            .build();
        let request = OpenRtpReceiver {
            params,
            playback_range: None,
        };
        let ctx = MediaRequestContext::default();
        let api = self.rtp_session_api()?;
        api.open_receiver(&ctx, request)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))
    }

    /// Open an RTP sender for the given GB session and return the descriptor.
    async fn open_gb_sender(
        &self,
        app: &str,
        stream: &str,
        ssrc: u32,
        remote: SocketAddr,
    ) -> Result<cheetah_sdk::media_api::rtp_session::RtpSessionDescriptor, SdkError> {
        let media_key = self.media_key(app, stream)?;
        let params = RtpSessionParamsBuilder::new(media_key, RtpDirection::Send)
            .transport(RtpTransport::Udp)
            .container(MediaContainer::Ps)
            .ssrc(ssrc)
            .payload_binding(self.ps_payload_binding())
            .remote_endpoint(remote)
            .build();
        let request = OpenRtpSender { params };
        let ctx = MediaRequestContext::default();
        let api = self.rtp_session_api()?;
        api.open_sender(&ctx, request)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))
    }

    /// Open a duplex voice-talk session and return the descriptor.
    async fn open_gb_talk(
        &self,
        app: &str,
        stream: &str,
        ssrc: u32,
        remote: SocketAddr,
        local_port: u16,
        payload_binding: RtpPayloadBinding,
    ) -> Result<cheetah_sdk::media_api::rtp_session::RtpSessionDescriptor, SdkError> {
        let media_key = self.media_key(app, stream)?;
        let local_endpoint_hint = SocketAddr::new(
            "0.0.0.0"
                .parse::<std::net::IpAddr>()
                .map_err(|e| SdkError::Internal(e.to_string()))?,
            local_port,
        );
        let params = RtpSessionParamsBuilder::new(media_key, RtpDirection::DuplexTalk)
            .transport(RtpTransport::Udp)
            .container(MediaContainer::ElementaryStream)
            .ssrc(ssrc)
            .payload_binding(payload_binding.clone())
            .remote_endpoint(remote)
            .local_endpoint_hint(local_endpoint_hint)
            .build();
        let request = OpenRtpTalk {
            params,
            talkback_binding: Some(payload_binding),
        };
        let ctx = MediaRequestContext::default();
        let api = self.rtp_session_api()?;
        api.open_talk(&ctx, request)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))
    }

    /// Stop a previously tracked RTP session and return whether it was found.
    async fn stop_gb_session(&self, session_ref: RtpSessionRef) -> Result<bool, SdkError> {
        let ctx = MediaRequestContext::default();
        let api = self.rtp_session_api()?;
        match api
            .stop_session(
                &ctx,
                StopRtpSession {
                    session_ref,
                    release_lease: true,
                },
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(e) if e.code == cheetah_sdk::media_api::error::MediaErrorCode::NotFound => {
                Ok(false)
            }
            Err(e) => Err(SdkError::Internal(e.to_string())),
        }
    }
}

/// `ModuleHttpService` implementation for GB28181 REST endpoints.
///
/// GB28181 REST 端点的 `ModuleHttpService` 实现。
#[async_trait]
impl ModuleHttpService for GbHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        match (req.method, req.path.as_str()) {
            (HttpMethod::Post, "/recv/create") => {
                let body: GbRecvRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;
                body.validate()?;

                let ssrc = body.ssrc();
                let port = body.port(self.default_media_port);
                let app = body.base.app;
                let stream = body.base.stream;

                // Allocate RTP server port and session in-process.
                // SIP INVITE/SDP negotiation is performed by the external signaling system.
                let descriptor = self.open_gb_receiver(&app, &stream, ssrc, port).await?;

                let session_key = format!("{app}/{stream}");
                let rtp_session_ref = RtpSessionRef {
                    session_id: descriptor.session_id.clone(),
                    expected_generation: descriptor.generation,
                };
                self.active_sessions
                    .lock()
                    .insert(session_key.clone(), (String::new(), rtp_session_ref));

                let local_port = descriptor.endpoints.local.port();

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                    "data": {
                        "port": local_port,
                        "ssrc": ssrc,
                        "sessionKey": session_key,
                    }
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            (HttpMethod::Post, "/recv/stop") => {
                let body: GbStopRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;
                body.validate()?;

                let app = body.base.app;
                let stream = body.base.stream;
                let session_key = format!("{app}/{stream}");

                let session_ref = {
                    let mut sessions = self.active_sessions.lock();
                    sessions.remove(&session_key).map(|(_, r)| r)
                };
                if let Some(session_ref) = session_ref {
                    self.stop_gb_session(session_ref).await.ok();
                }

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success"
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            (HttpMethod::Post, "/send/create") => {
                let body: GbSendRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;
                body.validate()?;

                let app = body.base.app;
                let stream = body.base.stream;
                let remote = format!("{}:{}", body.ip, body.port)
                    .parse::<SocketAddr>()
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid destination: {e}")))?;
                let ssrc = body.ssrc as u32;

                // Create RTP sender and start egress in one typed call.
                let descriptor = self.open_gb_sender(&app, &stream, ssrc, remote).await?;

                let session_key = format!("{app}/{stream}");
                let rtp_session_ref = RtpSessionRef {
                    session_id: descriptor.session_id.clone(),
                    expected_generation: descriptor.generation,
                };
                self.active_sessions
                    .lock()
                    .insert(session_key, (String::new(), rtp_session_ref));

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                    "data": {
                        "appName": app,
                        "streamName": stream,
                        "ssrc": ssrc,
                        "sessionKey": format!("{app}/{stream}")
                    }
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            (HttpMethod::Post, "/send/stop") => {
                let body: GbStopRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;
                body.validate()?;

                let app = body.base.app;
                let stream = body.base.stream;
                let session_key = format!("{app}/{stream}");
                let session_ref = {
                    let mut sessions = self.active_sessions.lock();
                    sessions.remove(&session_key).map(|(_, r)| r)
                };
                if let Some(session_ref) = session_ref {
                    self.stop_gb_session(session_ref).await.ok();
                }

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success"
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            (HttpMethod::Post, "/talk/start") => {
                let body: GbTalkRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;
                body.validate()?;

                let ssrc = body.ssrc();
                let local_port = body.local_port(self.default_media_port);
                let dest_addr = format!("{}:{}", body.ip, body.port)
                    .parse::<SocketAddr>()
                    .map_err(|e| {
                        SdkError::InvalidArgument(format!("invalid destination address: {e}"))
                    })?;
                let payload_binding = body.payload_binding();
                let app = body.base.app;
                let stream = body.base.stream;

                let descriptor = self
                    .open_gb_talk(&app, &stream, ssrc, dest_addr, local_port, payload_binding)
                    .await?;

                let session_key = format!("{app}/{stream}");
                let rtp_session_ref = RtpSessionRef {
                    session_id: descriptor.session_id.clone(),
                    expected_generation: descriptor.generation,
                };
                self.active_sessions
                    .lock()
                    .insert(session_key.clone(), (String::new(), rtp_session_ref));

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                    "data": {
                        "port": descriptor.endpoints.local.port(),
                        "ssrc": ssrc,
                        "sessionKey": session_key,
                    }
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            (HttpMethod::Post, "/talk/stop") => {
                let body: GbStopRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;
                body.validate()?;

                let app = body.base.app;
                let stream = body.base.stream;
                let session_key = format!("{app}/{stream}");

                let session_ref = {
                    let mut sessions = self.active_sessions.lock();
                    sessions.remove(&session_key).map(|(_, r)| r)
                };
                if let Some(session_ref) = session_ref {
                    self.stop_gb_session(session_ref).await.ok();
                }

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success"
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            _ => Err(SdkError::InvalidArgument("Not Found".to_string())),
        }
    }
}
