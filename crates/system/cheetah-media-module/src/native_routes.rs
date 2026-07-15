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
