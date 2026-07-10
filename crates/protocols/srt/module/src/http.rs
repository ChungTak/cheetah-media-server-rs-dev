use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::{HttpHeader, HttpMethod, HttpRequest, HttpResponse, ModuleHttpService, SdkError};

use crate::metrics::SrtModuleMetrics;

/// `SrtHttpService` data structure.
/// `SrtHttpService` 数据结构.
pub(crate) struct SrtHttpService {
    /// `metrics` field.
    /// `metrics` 字段.
    pub metrics: Arc<SrtModuleMetrics>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_srt_core::SrtStreamMode;

    #[test]
    fn prometheus_metrics_endpoint_renders_current_snapshot() {
        let metrics = SrtModuleMetrics::new();
        metrics.inc_connection(SrtStreamMode::Publish);
        metrics.inc_driver_error("SRT send queue full");
        let service = SrtHttpService { metrics };

        let response = futures::executor::block_on(service.handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/metrics".to_string(),
            query: None,
            headers: Vec::new(),
            body: Default::default(),
        }))
        .expect("metrics response");

        assert_eq!(response.status, 200);
        let body = std::str::from_utf8(&response.body).expect("metrics body utf8");
        assert!(body.contains("# TYPE srt_connections_active gauge"));
        assert!(body.contains("srt_connections_active 1"));
        assert!(body.contains("srt_send_queue_full_total 1"));
    }

    #[test]
    fn unknown_http_route_returns_404() {
        let service = SrtHttpService {
            metrics: SrtModuleMetrics::new(),
        };

        let response = futures::executor::block_on(service.handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/unknown".to_string(),
            query: None,
            headers: Vec::new(),
            body: Default::default(),
        }))
        .expect("404 response");

        assert_eq!(response.status, 404);
    }
}

#[async_trait]
impl ModuleHttpService for SrtHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        match (req.method, req.path.as_str()) {
            (HttpMethod::Get, "/metrics") => Ok(self.handle_metrics()),
            (HttpMethod::Get, "/metrics.json") => self.handle_metrics_json(),
            _ => Ok(HttpResponse {
                status: 404,
                headers: Vec::new(),
                body: Default::default(),
            }),
        }
    }
}

impl SrtHttpService {
    fn handle_metrics(&self) -> HttpResponse {
        let snapshot = self.metrics.snapshot();
        let mut out = String::with_capacity(1024);
        macro_rules! gauge {
            ($name:literal, $help:literal, $value:expr) => {
                out.push_str(&format!("# HELP {} {}\n", $name, $help));
                out.push_str(&format!("# TYPE {} gauge\n", $name));
                out.push_str(&format!("{} {}\n", $name, $value));
            };
        }
        macro_rules! counter {
            ($name:literal, $help:literal, $value:expr) => {
                out.push_str(&format!("# HELP {} {}\n", $name, $help));
                out.push_str(&format!("# TYPE {} counter\n", $name));
                out.push_str(&format!("{} {}\n", $name, $value));
            };
        }

        gauge!(
            "srt_connections_active",
            "Active SRT connections.",
            snapshot.connections_active
        );
        counter!(
            "srt_connections_total",
            "Total accepted or established SRT connections.",
            snapshot.connections_total
        );
        counter!(
            "srt_publish_connections_total",
            "Total SRT publish-side connections.",
            snapshot.publish_connections_total
        );
        counter!(
            "srt_play_connections_total",
            "Total SRT request/play-side connections.",
            snapshot.play_connections_total
        );
        counter!(
            "srt_bytes_in_total",
            "Total SRT bytes received by the driver.",
            snapshot.bytes_in_total
        );
        counter!(
            "srt_bytes_out_total",
            "Total SRT bytes sent by the driver.",
            snapshot.bytes_out_total
        );
        counter!(
            "srt_packets_in_total",
            "Total SRT packets received by the driver.",
            snapshot.packets_in_total
        );
        counter!(
            "srt_packets_out_total",
            "Total SRT packets sent by the driver.",
            snapshot.packets_out_total
        );
        counter!(
            "srt_retransmit_total",
            "Total SRT retransmits observed by the sender.",
            snapshot.retransmit_total
        );
        counter!(
            "srt_lost_packets_total",
            "Total SRT lost packets observed by the receiver.",
            snapshot.lost_packets_total
        );
        counter!(
            "srt_duplicate_packets_total",
            "Total duplicate SRT packets observed by the receiver.",
            snapshot.duplicate_packets_total
        );
        gauge!(
            "srt_send_queue_depth",
            "Current SRT sender buffer packet depth.",
            snapshot.send_queue_depth
        );
        gauge!(
            "srt_recv_queue_depth",
            "Current SRT receiver buffer packet depth.",
            snapshot.recv_queue_depth
        );
        gauge!(
            "srt_rtt_micros",
            "Last observed SRT receiver RTT in microseconds.",
            snapshot.rtt_micros
        );
        gauge!(
            "srt_jitter_micros",
            "Last observed SRT receiver jitter in microseconds.",
            snapshot.jitter_micros
        );
        counter!(
            "srt_key_refresh_total",
            "Total SRT key refresh notifications.",
            snapshot.key_refresh_total
        );
        counter!(
            "srt_disconnect_total",
            "Total SRT disconnect events.",
            snapshot.disconnect_total
        );
        counter!(
            "srt_driver_errors_total",
            "Total SRT driver errors.",
            snapshot.driver_errors_total
        );
        counter!(
            "srt_send_queue_full_total",
            "Total SRT payloads rejected because the send queue was full.",
            snapshot.send_queue_full_total
        );

        HttpResponse {
            status: 200,
            headers: vec![HttpHeader {
                name: "content-type".to_string(),
                value: "text/plain; version=0.0.4; charset=utf-8".to_string(),
            }],
            body: out.into(),
        }
    }

    fn handle_metrics_json(&self) -> Result<HttpResponse, SdkError> {
        serde_json::to_vec(&self.metrics.snapshot())
            .map(HttpResponse::ok_json)
            .map_err(|err| SdkError::Internal(format!("render SRT metrics failed: {err}")))
    }
}
