use cheetah_media_api::MediaScope;
use cheetah_sdk::{HttpMethod, HttpRouteDescriptor};

/// Delivery level of a ZLM-compatible route.
///
/// ZLM 兼容路由的交付级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZlmRouteLevel {
    /// Core media capability with a production success path.
    L1,
    /// Optional provider; real if the provider is registered, otherwise `-501`.
    L2,
    /// Admin-guarded operation; requires `server.admin` scope.
    L3,
    /// Out-of-scope compatibility placeholder; route exists but returns `-501`.
    L4,
}

#[allow(dead_code)]
struct ZlmRoute {
    method: HttpMethod,
    path: &'static str,
    scope: MediaScope,
    level: ZlmRouteLevel,
    /// Whether this route is part of the 64-route required catalog in
    /// `dev-docs/901_api_plan/05_zlm_http_adapter.md`.
    required: bool,
}

/// Catalog of all ZLM-compatible `/index/api/*` routes.
///
/// The first 64 entries are the required catalog (sections 3.1–3.8 of
/// `05_zlm_http_adapter.md`). Additional optional routes are marked with
/// `required: false`.
const ZLM_ROUTES: &[ZlmRoute] = &[
    // 3.1 系统与配置 (L3)
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getThreadsLoad",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getWorkThreadsLoad",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getServerConfig",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/setServerConfig",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getApiList",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/version",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/restartServer",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: true,
    },
    // 3.2 媒体、会话和广播 (L1 + L2)
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getMediaList",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/isMediaOnline",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getMediaPlayerList",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getMediaInfo",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/close_stream",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/close_streams",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getAllSession",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/kick_session",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/kick_sessions",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/broadcastMessage",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    // 3.3 拉流、推流和 FFmpeg 代理 (L1 + L2)
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/addStreamProxy",
        scope: MediaScope::MediaPublish,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/delStreamProxy",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/listStreamProxy",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getProxyInfo",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/addStreamPusherProxy",
        scope: MediaScope::MediaPublish,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/delStreamPusherProxy",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/listStreamPusherProxy",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getProxyPusherInfo",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/addFFmpegSource",
        scope: MediaScope::MediaPublish,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/delFFmpegSource",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/listFFmpegSource",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    // 3.4 RTP server/client (L1 + L2)
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getRtpInfo",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/openRtpServer",
        scope: MediaScope::MediaPublish,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/openRtpServerMultiplex",
        scope: MediaScope::MediaPublish,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/connectRtpServer",
        scope: MediaScope::MediaPublish,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/closeRtpServer",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/updateRtpServerSSRC",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/listRtpServer",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/pauseRtpCheck",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/resumeRtpCheck",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/startSendRtp",
        scope: MediaScope::MediaConsume,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/startSendRtpPassive",
        scope: MediaScope::MediaConsume,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/startSendRtpTalk",
        scope: MediaScope::MediaConsume,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/listRtpSender",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/stopSendRtp",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    // 3.5 录制与文件 (L1 + L2)
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/startRecord",
        scope: MediaScope::RecordManage,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/startRecordTask",
        scope: MediaScope::RecordManage,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/setRecordSpeed",
        scope: MediaScope::RecordManage,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/seekRecordStamp",
        scope: MediaScope::RecordManage,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/stopRecord",
        scope: MediaScope::RecordManage,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/isRecording",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getMP4RecordFile",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/deleteRecordDirectory",
        scope: MediaScope::FileDelete,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/loadMP4File",
        scope: MediaScope::RecordManage,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/controlRecordPlay",
        scope: MediaScope::RecordManage,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    // 3.6 快照与文件下载 (L1)
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getSnap",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/deleteSnapDirectory",
        scope: MediaScope::FileDelete,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/downloadFile",
        scope: MediaScope::FileRead,
        level: ZlmRouteLevel::L1,
        required: true,
    },
    // 3.7 WebRTC/WHIP/WHEP (L2)
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/webrtc",
        scope: MediaScope::MediaPublish,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/whip",
        scope: MediaScope::MediaPublish,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/whep",
        scope: MediaScope::MediaConsume,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/delete_webrtc",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getWebrtcProxyPlayerInfo",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    // 3.8 WebRTC room keeper (L2)
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/addWebrtcRoomKeeper",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/delWebrtcRoomKeeper",
        scope: MediaScope::MediaControl,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/listWebrtcRoomKeepers",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/listWebrtcRooms",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L2,
        required: true,
    },
    // 3.9 其他可选 API (L3/L4, not part of the 64-route required catalog)
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/login",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L3,
        required: false,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/logout",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L3,
        required: false,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/searchOnvifDevice",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L4,
        required: false,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/getStreamUrl",
        scope: MediaScope::MediaRead,
        level: ZlmRouteLevel::L2,
        required: false,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/addProbe",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: false,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/stack/start",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: false,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/stack/reset",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: false,
    },
    ZlmRoute {
        method: HttpMethod::Post,
        path: "/api/stack/stop",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: false,
    },
    ZlmRoute {
        method: HttpMethod::Get,
        path: "/api/downloadBin",
        scope: MediaScope::ServerAdmin,
        level: ZlmRouteLevel::L3,
        required: false,
    },
];

/// Return all ZLM-compatible HTTP route descriptors.
///
/// 返回所有 ZLM 兼容 HTTP 路由描述符。
pub fn zlm_http_routes() -> Vec<HttpRouteDescriptor> {
    ZLM_ROUTES
        .iter()
        .map(|r| HttpRouteDescriptor {
            method: r.method,
            path: r.path.to_string(),
        })
        .collect()
}

/// Return the required authorization scope for a ZLM route, if it exists in the catalog.
///
/// 返回 ZLM 路由所需的授权 scope（如果该路由在目录中）。
pub fn zlm_required_scope(method: HttpMethod, path: &str) -> Option<MediaScope> {
    ZLM_ROUTES
        .iter()
        .find(|r| r.method == method && r.path == path)
        .map(|r| r.scope.clone())
}

/// Return the delivery level for a ZLM route, if it exists in the catalog.
///
/// 返回 ZLM 路由的交付级别（如果该路由在目录中）。
#[allow(dead_code)]
pub fn zlm_route_level(method: HttpMethod, path: &str) -> Option<ZlmRouteLevel> {
    ZLM_ROUTES
        .iter()
        .find(|r| r.method == method && r.path == path)
        .map(|r| r.level)
}

/// Return `true` if the method/path pair is a known ZLM-compatible route.
///
/// 如果方法/路径对是已知的 ZLM 兼容路由，则返回 `true`。
pub fn is_zlm_catalog_route(method: HttpMethod, path: &str) -> bool {
    ZLM_ROUTES
        .iter()
        .any(|r| r.method == method && r.path == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zlm_catalog_contains_64_required_routes() {
        let required = ZLM_ROUTES.iter().filter(|r| r.required).count();
        assert_eq!(
            required, 64,
            "required ZLM route catalog must contain 64 routes"
        );
    }

    #[test]
    fn zlm_required_scope_is_defined_for_every_route() {
        for route in ZLM_ROUTES {
            let scope = zlm_required_scope(route.method, route.path);
            assert!(
                scope.is_some(),
                "{:?} {} must have a required scope",
                route.method,
                route.path
            );
        }
    }

    #[test]
    fn zlm_catalog_has_no_duplicate_routes() {
        let mut seen: Vec<(HttpMethod, &str)> = Vec::new();
        for route in ZLM_ROUTES {
            let key = (route.method, route.path);
            assert!(
                !seen.contains(&key),
                "duplicate route: {:?} {}",
                route.method,
                route.path
            );
            seen.push(key);
        }
        assert_eq!(seen.len(), ZLM_ROUTES.len());
    }

    #[test]
    fn zlm_route_levels_are_classified() {
        let l1 = ZLM_ROUTES
            .iter()
            .filter(|r| matches!(r.level, ZlmRouteLevel::L1))
            .count();
        let l2 = ZLM_ROUTES
            .iter()
            .filter(|r| matches!(r.level, ZlmRouteLevel::L2))
            .count();
        let l3 = ZLM_ROUTES
            .iter()
            .filter(|r| matches!(r.level, ZlmRouteLevel::L3))
            .count();
        let l4 = ZLM_ROUTES
            .iter()
            .filter(|r| matches!(r.level, ZlmRouteLevel::L4))
            .count();
        assert!(l1 > 0, "L1 routes must exist");
        assert!(l2 > 0, "L2 routes must exist");
        assert!(l3 > 0, "L3 routes must exist");
        assert!(l4 > 0, "L4 routes must exist");
        for route in ZLM_ROUTES {
            assert_eq!(
                zlm_route_level(route.method, route.path),
                Some(route.level),
                "{:?} {} level mismatch",
                route.method,
                route.path
            );
        }
    }

    #[test]
    fn zlm_http_routes_matches_catalog() {
        let routes = zlm_http_routes();
        assert_eq!(routes.len(), ZLM_ROUTES.len());
        for (expected, actual) in ZLM_ROUTES.iter().zip(routes.iter()) {
            assert_eq!(expected.method, actual.method);
            assert_eq!(expected.path, actual.path);
        }
    }

    #[test]
    fn unknown_route_is_not_in_catalog() {
        assert!(!is_zlm_catalog_route(HttpMethod::Get, "/api/unknown"));
        assert!(zlm_required_scope(HttpMethod::Get, "/api/unknown").is_none());
    }
}
