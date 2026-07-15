use cheetah_media_api::MediaScope;
use cheetah_sdk::{HttpMethod, HttpRouteDescriptor};

/// Native `/api/v1` HTTP route catalog.
///
/// Paths use `{name}` placeholders for single dynamic segments. The control-plane
/// dispatcher (`cheetah-control`) matches these templates before invoking the
/// module handler, so the module no longer unconditionally takes over all
/// requests under its mount prefix.
pub fn native_http_routes() -> Vec<HttpRouteDescriptor> {
    vec![
        // media
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/media/capabilities".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/media".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/media/{vhost}/{app}/{stream}".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/media/{vhost}/{app}/{stream}/online".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/media/{vhost}/{app}/{stream}/close".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/media/{vhost}/{app}/{stream}/keyframe".to_string(),
        },
        // sessions
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/sessions".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/sessions/{session_id}/kick".to_string(),
        },
        // record
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/record/tasks".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/record/tasks".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/record/tasks/{task_id}/stop".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/record/files".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Delete,
            path: "/record/files/{file_id}".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/record/playback/{file_id}/control".to_string(),
        },
        // snapshots
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/snapshots".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/snapshots".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Delete,
            path: "/snapshots/directories".to_string(),
        },
        // file store
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/files/{file_id}/download".to_string(),
        },
        // proxies
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/proxies/pull".to_string(),
        },
        // rtp
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/rtp/receivers".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/rtp/senders".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Get,
            path: "/rtp/sessions".to_string(),
        },
        HttpRouteDescriptor {
            method: HttpMethod::Post,
            path: "/rtp/sessions/{session_id}/stop".to_string(),
        },
    ]
}

/// Return the required authorization scope for a native route, if any.
///
/// 返回 native 路由所需的授权 scope（如有）。
pub fn native_required_scope(method: HttpMethod, path: &str) -> Option<MediaScope> {
    match (method, path) {
        (HttpMethod::Get, "/media/capabilities") => Some(MediaScope::MediaRead),
        (HttpMethod::Get, "/media") => Some(MediaScope::MediaRead),
        (HttpMethod::Get, _) if path.starts_with("/media/") && path.ends_with("/online") => {
            Some(MediaScope::MediaRead)
        }
        (HttpMethod::Get, _) if path.starts_with("/media/") => Some(MediaScope::MediaRead),
        (HttpMethod::Post, _) if path.starts_with("/media/") && path.ends_with("/close") => {
            Some(MediaScope::MediaControl)
        }
        (HttpMethod::Post, _) if path.starts_with("/media/") && path.ends_with("/keyframe") => {
            Some(MediaScope::MediaControl)
        }
        (HttpMethod::Get, "/sessions") => Some(MediaScope::MediaRead),
        (HttpMethod::Post, _) if path.starts_with("/sessions/") && path.ends_with("/kick") => {
            Some(MediaScope::MediaControl)
        }
        (HttpMethod::Get, "/record/tasks") => Some(MediaScope::MediaRead),
        (HttpMethod::Post, "/record/tasks") => Some(MediaScope::RecordManage),
        (HttpMethod::Post, _) if path.starts_with("/record/tasks/") && path.ends_with("/stop") => {
            Some(MediaScope::RecordManage)
        }
        (HttpMethod::Get, "/record/files") => Some(MediaScope::MediaRead),
        (HttpMethod::Delete, _) if path.starts_with("/record/files/") => {
            Some(MediaScope::FileDelete)
        }
        (HttpMethod::Post, _)
            if path.starts_with("/record/playback/") && path.ends_with("/control") =>
        {
            Some(MediaScope::RecordManage)
        }
        (HttpMethod::Post, "/snapshots") => Some(MediaScope::MediaControl),
        (HttpMethod::Get, "/snapshots") => Some(MediaScope::MediaRead),
        (HttpMethod::Delete, "/snapshots/directories") => Some(MediaScope::FileDelete),
        (HttpMethod::Get, _) if path.starts_with("/files/") && path.ends_with("/download") => {
            Some(MediaScope::FileRead)
        }
        (HttpMethod::Get, "/proxies/pull") => Some(MediaScope::MediaPublish),
        (HttpMethod::Post, "/rtp/receivers") => Some(MediaScope::MediaPublish),
        (HttpMethod::Post, "/rtp/senders") => Some(MediaScope::MediaConsume),
        (HttpMethod::Get, "/rtp/sessions") => Some(MediaScope::MediaRead),
        (HttpMethod::Post, _) if path.starts_with("/rtp/sessions/") && path.ends_with("/stop") => {
            Some(MediaScope::MediaControl)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_routes_require_media_read() {
        assert_eq!(
            native_required_scope(HttpMethod::Get, "/media"),
            Some(MediaScope::MediaRead)
        );
        assert_eq!(
            native_required_scope(HttpMethod::Get, "/media/live/test/stream"),
            Some(MediaScope::MediaRead)
        );
        assert_eq!(
            native_required_scope(HttpMethod::Get, "/media/live/test/stream/online"),
            Some(MediaScope::MediaRead)
        );
        assert_eq!(
            native_required_scope(HttpMethod::Get, "/sessions"),
            Some(MediaScope::MediaRead)
        );
    }

    #[test]
    fn control_routes_require_media_control_or_higher() {
        assert_eq!(
            native_required_scope(HttpMethod::Post, "/media/live/test/stream/close"),
            Some(MediaScope::MediaControl)
        );
        assert_eq!(
            native_required_scope(HttpMethod::Post, "/media/live/test/stream/keyframe"),
            Some(MediaScope::MediaControl)
        );
        assert_eq!(
            native_required_scope(HttpMethod::Post, "/sessions/uuid/kick"),
            Some(MediaScope::MediaControl)
        );
        assert_eq!(
            native_required_scope(HttpMethod::Post, "/rtp/sessions/uuid/stop"),
            Some(MediaScope::MediaControl)
        );
    }

    #[test]
    fn record_manage_and_file_scopes_are_distinct() {
        assert_eq!(
            native_required_scope(HttpMethod::Post, "/record/tasks"),
            Some(MediaScope::RecordManage)
        );
        assert_eq!(
            native_required_scope(HttpMethod::Post, "/record/tasks/uuid/stop"),
            Some(MediaScope::RecordManage)
        );
        assert_eq!(
            native_required_scope(HttpMethod::Get, "/record/files"),
            Some(MediaScope::MediaRead)
        );
        assert_eq!(
            native_required_scope(HttpMethod::Delete, "/record/files/uuid"),
            Some(MediaScope::FileDelete)
        );
        assert_eq!(
            native_required_scope(HttpMethod::Post, "/record/playback/uuid/control"),
            Some(MediaScope::RecordManage)
        );
    }

    #[test]
    fn unknown_routes_have_no_required_scope() {
        assert_eq!(native_required_scope(HttpMethod::Get, "/unknown"), None);
    }
}
