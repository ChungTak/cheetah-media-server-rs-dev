//! HTTP route table for the ZLMediaKit-compatible adapter.
//!
//! 为 ZLMediaKit 兼容适配器实现的 HTTP 路由表。

use cheetah_sdk::{HttpMethod, HttpRouteDescriptor};

pub(crate) fn http_routes() -> Vec<HttpRouteDescriptor> {
    vec![
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/api/getMediaList".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/api/isMediaOnline".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/api/getMediaInfo".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/api/getAllSession".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/close_stream".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/kick_session".to_string(),
        },
        // Record endpoints; detailed implementation in record module / future media provider.
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/startRecord".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/stopRecord".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/api/isRecording".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/api/getMP4RecordFile".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/deleteRecordDirectory".to_string(),
        },
        // RTP endpoints
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/openRtpServer".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/closeRtpServer".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/startSendRtp".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/stopSendRtp".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/api/getRtpInfo".to_string(),
        },
        // Proxy endpoints
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/addStreamProxy".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/delStreamProxy".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/api/getAllStreamProxy".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/addFFmpegSource".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/delFFmpegSource".to_string(),
        },
        // Server ops endpoints
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/api/getServerLoad".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/api/getWorkThreadsLoad".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/api/getServerConfig".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/setServerConfig".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/restartServer".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/api/shutdownServer".to_string(),
        },
    ]
}
