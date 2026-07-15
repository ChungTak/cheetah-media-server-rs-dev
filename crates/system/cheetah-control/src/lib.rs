use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::{Extension, Path, Request};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use cheetah_sdk::{
    ConfigApplyApi, ConfigEffect, ConfigProvider, ConfigSchemaRegistry, HttpMethod, HttpRequest,
    HttpResponse, HttpRouteMount, ModuleId, ModuleManagerApi, ServiceRegistry, StreamManagerApi,
    TaskSystemApi,
};
use cheetah_sdk::{HealthApi, MetricsApi};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone)]
/// Shared runtime APIs used by the HTTP control plane.
///
/// HTTP 控制平面使用的共享运行时 API。
pub struct ControlState {
    pub health: Arc<dyn HealthApi>,
    pub metrics: Arc<dyn MetricsApi>,
    pub modules: Arc<dyn ModuleManagerApi>,
    pub streams: Arc<dyn StreamManagerApi>,
    pub tasks: Arc<dyn TaskSystemApi>,
    pub config: Arc<dyn ConfigProvider>,
    pub config_apply: Arc<dyn ConfigApplyApi>,
    pub config_schemas: Arc<dyn ConfigSchemaRegistry>,
    pub service_registry: Arc<dyn ServiceRegistry>,
}

#[derive(Debug, Deserialize)]
/// Config patch request body with optional effect hint.
///
/// 配置补丁请求体，带可选效果提示。
struct PatchRequest {
    patch: Value,
    effect: Option<String>,
}

/// Build the Axum router for the control plane endpoints.
///
/// 为控制平面端点构建 Axum 路由。
pub fn router(state: ControlState) -> Router {
    let state = Arc::new(state);
    Router::new()
        .route("/healthz", get(get_healthz))
        .route("/readyz", get(get_readyz))
        .route("/metrics", get(get_metrics))
        .route("/api/v1/modules", get(get_modules))
        .route("/api/v1/streams", get(get_streams))
        .route("/api/v1/tasks", get(get_tasks))
        .route("/api/v1/services", get(get_services))
        .route("/api/v1/config", get(get_config).patch(patch_global_config))
        .route("/api/v1/config/schemas", get(get_config_schemas))
        .route(
            "/api/v1/config/modules/:module_id",
            patch(patch_module_config),
        )
        .fallback(handle_module_http)
        .layer(Extension(state))
}

/// Bind an HTTP server to the given address and serve the control plane.
///
/// 将 HTTP 服务器绑定到指定地址并为控制平面提供服务。
pub fn spawn_server(
    addr: SocketAddr,
    state: ControlState,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router(state)).await?;
        Ok(())
    })
}

/// Return HTTP 200 if the engine is live, otherwise 503.
///
/// 若引擎存活则返回 200，否则返回 503。
async fn get_healthz(Extension(state): Extension<Arc<ControlState>>) -> impl IntoResponse {
    if state.health.is_live() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Return HTTP 200 if the engine is ready, otherwise 503.
///
/// 若引擎就绪则返回 200，否则返回 503。
async fn get_readyz(Extension(state): Extension<Arc<ControlState>>) -> impl IntoResponse {
    if state.health.is_ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Render engine metrics as plain text.
///
/// 以纯文本渲染引擎指标。
async fn get_metrics(Extension(state): Extension<Arc<ControlState>>) -> impl IntoResponse {
    (StatusCode::OK, state.metrics.render())
}

/// List modules, their states, and their mounted HTTP route prefixes.
///
/// 列出模块、状态及其挂载的 HTTP 路由前缀。
async fn get_modules(Extension(state): Extension<Arc<ControlState>>) -> impl IntoResponse {
    let mounted = state.modules.http_mounts();
    let modules = state
        .modules
        .modules()
        .into_iter()
        .map(|(module_id, module_state)| {
            let route_prefixes = mounted
                .iter()
                .filter(|mount| mount.module_id == module_id)
                .map(|mount| mount.prefix.clone())
                .collect::<Vec<_>>();
            json!({
                "module_id": module_id.0,
                "state": format!("{:?}", module_state),
                "http_prefixes": route_prefixes,
            })
        })
        .collect::<Vec<_>>();
    Json(json!({"modules": modules}))
}

/// List active streams and their track metadata.
///
/// 列出活跃流及其轨道元数据。
async fn get_streams(Extension(state): Extension<Arc<ControlState>>) -> impl IntoResponse {
    match state.streams.list_streams().await {
        Ok(streams) => {
            let list = streams
                .into_iter()
                .map(|stream| {
                    json!({
                        "stream_id": stream.stream_id.0,
                        "namespace": stream.key.namespace,
                        "path": stream.key.path,
                        "publisher_active": stream.publisher_active,
                        "subscriber_count": stream.subscriber_count,
                        "tracks": stream
                            .tracks
                            .into_iter()
                            .map(|track| {
                                json!({
                                    "track_id": track.track_id.0,
                                    "media_kind": format!("{:?}", track.media_kind),
                                    "codec": format!("{:?}", track.codec),
                                    "payload_type": track.payload_type,
                                    "clock_rate": track.clock_rate,
                                    "sample_rate": track.sample_rate,
                                    "channels": track.channels,
                                    "width": track.width,
                                    "height": track.height,
                                    "fps": track.fps.map(|v| json!({"num": v.num, "den": v.den})),
                                    "bitrate": track.bitrate,
                                    "readiness": format!("{:?}", track.readiness),
                                })
                            })
                            .collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>();
            (StatusCode::OK, Json(json!({"streams": list}))).into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": err.to_string()})),
        )
            .into_response(),
    }
}

/// List all task snapshots with their tree structure.
///
/// 列出所有任务快照及其树结构。
async fn get_tasks(Extension(state): Extension<Arc<ControlState>>) -> impl IntoResponse {
    let tasks = state
        .tasks
        .snapshot()
        .into_iter()
        .map(|snapshot| {
            json!({
                "id": snapshot.id.0,
                "parent_id": snapshot.parent_id.map(|id| id.0),
                "kind": format!("{:?}", snapshot.kind),
                "state": format!("{:?}", snapshot.state),
                "terminal_outcome": snapshot.terminal_outcome.map(|v| format!("{:?}", v)),
                "owner": snapshot.owner,
                "label": snapshot.label,
                "level": snapshot.level,
                "started_unix_millis": snapshot.started_unix_millis,
                "updated_unix_millis": snapshot.updated_unix_millis,
                "finished_unix_millis": snapshot.finished_unix_millis,
                "cancel_reason": snapshot.cancel_reason,
                "finish_message": snapshot.finish_message,
                "spawn_site": snapshot.spawn_site,
                "children": snapshot.child_ids.into_iter().map(|id| id.0).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    Json(json!({"tasks": tasks}))
}

/// List all registered service descriptors.
///
/// 列出所有已注册服务描述符。
async fn get_services(Extension(state): Extension<Arc<ControlState>>) -> impl IntoResponse {
    let services = state
        .service_registry
        .list_services()
        .into_iter()
        .map(|svc| {
            json!({
                "name": svc.name,
                "endpoint": svc.endpoint,
                "metadata": svc.metadata,
            })
        })
        .collect::<Vec<_>>();
    Json(json!({ "services": services }))
}

/// Return the current config version and global effective value.
///
/// 返回当前配置版本与全局有效值。
async fn get_config(Extension(state): Extension<Arc<ControlState>>) -> impl IntoResponse {
    Json(json!({
        "version": state.config.version(),
        "global": state.config.global(),
    }))
}

/// List all registered config schemas.
///
/// 列出所有已注册的配置 schema。
async fn get_config_schemas(Extension(state): Extension<Arc<ControlState>>) -> impl IntoResponse {
    let schemas = state
        .config_schemas
        .list_schemas()
        .into_iter()
        .map(|schema| {
            json!({
                "scope": schema.scope,
                "schema_name": schema.schema_name,
            })
        })
        .collect::<Vec<_>>();
    Json(json!({ "schemas": schemas }))
}

/// Apply a global config patch and forward module changes to `ModuleManagerApi`.
///
/// 应用全局配置补丁，并将模块变更转发给 `ModuleManagerApi`。
async fn patch_global_config(
    Extension(state): Extension<Arc<ControlState>>,
    Json(req): Json<PatchRequest>,
) -> impl IntoResponse {
    let effect = parse_effect(req.effect.as_deref());

    let applied = match state.config_apply.apply_global_patch(req.patch, effect) {
        Ok(v) => v,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": err.to_string()})),
            )
                .into_response();
        }
    };

    let reports = if applied.effect == ConfigEffect::EngineRestartRequired {
        Vec::new()
    } else {
        match state
            .modules
            .apply_module_config_changes(applied.module_changes.clone())
            .await
        {
            Ok(v) => v,
            Err(err) => {
                let rollback_err = applied
                    .rollback_token
                    .clone()
                    .map(|token| state.config_apply.rollback(token))
                    .transpose()
                    .err();
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": match rollback_err {
                        Some(rollback_err) => format!("module apply failed: {err}; config rollback failed: {rollback_err}"),
                        None => format!("module apply failed: {err}; config rolled back"),
                    }})),
                )
                    .into_response();
            }
        }
    };

    let reports = reports
        .into_iter()
        .map(|report| {
            json!({
                "module_id": report.module_id.0,
                "effect": format!("{:?}", report.effect),
            })
        })
        .collect::<Vec<_>>();

    (
        StatusCode::OK,
        Json(json!({
            "version": applied.version,
            "effect": format!("{:?}", applied.effect),
            "module_apply_reports": reports,
        })),
    )
        .into_response()
}

/// Apply a module config patch and forward the resulting change to `ModuleManagerApi`.
///
/// 应用模块配置补丁，并将结果变更转发给 `ModuleManagerApi`。
async fn patch_module_config(
    Extension(state): Extension<Arc<ControlState>>,
    Path(module_id): Path<String>,
    Json(req): Json<PatchRequest>,
) -> impl IntoResponse {
    let effect = parse_effect(req.effect.as_deref());

    let module_id = ModuleId::new(module_id);
    let applied = match state
        .config_apply
        .apply_module_patch(&module_id, req.patch, effect)
    {
        Ok(v) => v,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": err.to_string()})),
            )
                .into_response();
        }
    };

    let report = if applied.effect == ConfigEffect::EngineRestartRequired {
        None
    } else {
        match state
            .modules
            .apply_module_config_changes(applied.module_changes.clone())
            .await
        {
            Ok(mut v) => v.pop(),
            Err(err) => {
                let rollback_err = applied
                    .rollback_token
                    .clone()
                    .map(|token| state.config_apply.rollback(token))
                    .transpose()
                    .err();
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": match rollback_err {
                        Some(rollback_err) => format!("module apply failed: {err}; config rollback failed: {rollback_err}"),
                        None => format!("module apply failed: {err}; config rolled back"),
                    }})),
                )
                    .into_response();
            }
        }
    };

    (
        StatusCode::OK,
        Json(json!({
            "version": applied.version,
            "effect": format!("{:?}", applied.effect),
            "module_apply_report": report.map(|v| json!({
                "module_id": v.module_id.0,
                "effect": format!("{:?}", v.effect),
            })),
        })),
    )
        .into_response()
}

/// Dispatch a request to a module HTTP handler based on prefix and route matching.
///
/// 根据前缀与路由匹配将请求分派给模块 HTTP 处理器。
async fn handle_module_http(
    Extension(state): Extension<Arc<ControlState>>,
    req: Request<Body>,
) -> Response {
    let method = match to_sdk_method(req.method()) {
        Some(v) => v,
        None => return (StatusCode::METHOD_NOT_ALLOWED, "method not allowed").into_response(),
    };
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(ToString::to_string);
    let mounts = state.modules.http_mounts();

    let mut selected: Option<(HttpRouteMount, String)> = None;
    let mut method_mismatch = false;
    for mount in &mounts {
        let Some(relative_path) = relative_path(&mount.prefix, &path) else {
            continue;
        };
        let (matched, allowed) = route_match(mount, method, &relative_path);
        if matched && !allowed {
            method_mismatch = true;
            continue;
        }
        if !allowed {
            continue;
        }
        let should_replace = match &selected {
            Some((current, _)) => mount.prefix.len() > current.prefix.len(),
            None => true,
        };
        if should_replace {
            selected = Some((mount.clone(), relative_path));
        }
    }

    if selected.is_none() {
        for mount in &mounts {
            let (matched, allowed, module_path) = root_route_match(mount, method, &path);
            if matched && !allowed {
                method_mismatch = true;
                continue;
            }
            if !allowed {
                continue;
            }
            let should_replace = match &selected {
                Some((current, _)) => mount.prefix.len() > current.prefix.len(),
                None => true,
            };
            if should_replace {
                selected = Some((mount.clone(), module_path));
            }
        }
    }

    let Some((mount, relative)) = selected else {
        if method_mismatch {
            return (StatusCode::METHOD_NOT_ALLOWED, "method not allowed").into_response();
        }
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    let headers = req
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str().ok().map(|value| cheetah_sdk::HttpHeader {
                name: k.as_str().to_string(),
                value: value.to_string(),
            })
        })
        .collect::<Vec<_>>();

    let body = match to_bytes(req.into_body(), 8 * 1024 * 1024).await {
        Ok(v) => v,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("read request body failed: {err}"),
            )
                .into_response();
        }
    };

    let module_req = HttpRequest {
        method,
        path: relative,
        query,
        headers,
        body,
    };
    let module_resp = match mount.service.handle(module_req).await {
        Ok(v) => v,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": err.to_string()})),
            )
                .into_response();
        }
    };
    to_axum_response(module_resp)
}

/// Convert an `HttpResponse` into an Axum `Response`.
///
/// 将 `HttpResponse` 转换为 Axum `Response`。
fn to_axum_response(resp: HttpResponse) -> Response {
    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut builder = axum::http::Response::builder().status(status);
    for header in resp.headers {
        builder = builder.header(header.name, header.value);
    }
    builder
        .body(Body::from(resp.body))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "invalid response").into_response())
}

/// Normalize a path: trim, ensure leading slash, and strip trailing slashes.
///
/// 规范化路径：去除空白、确保前导斜杠、去除尾部斜杠。
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }
    let mut out = trimmed.to_string();
    if !out.starts_with('/') {
        out.insert(0, '/');
    }
    while out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// Compute the path relative to a prefix, or `None` if the prefix does not match.
///
/// 计算相对前缀的路径；若前缀不匹配则返回 `None`。
fn relative_path(prefix: &str, absolute_path: &str) -> Option<String> {
    let prefix = normalize_path(prefix);
    let absolute = normalize_path(absolute_path);
    if prefix == "/" {
        return Some(absolute);
    }
    if absolute == prefix {
        return Some("/".to_string());
    }
    absolute
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(|rest| {
            if rest.is_empty() {
                "/".to_string()
            } else {
                format!("/{}", rest)
            }
        })
}

/// Match a path template against an actual path.
///
/// A template segment wrapped in braces, e.g. `{id}`, matches any non-empty
/// single segment. All other segments must match literally.
///
/// 将路径模板与实际路径匹配。`{id}` 形式匹配任意非空单一段落，其它段落必须字面相等。
fn path_template_match(template: &str, path: &str) -> bool {
    let template_segments: Vec<&str> = template.split('/').filter(|s| !s.is_empty()).collect();
    let path_segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if template_segments.len() != path_segments.len() {
        return false;
    }
    for (t, p) in template_segments.iter().zip(path_segments.iter()) {
        if t.starts_with('{') && t.ends_with('}') {
            let name = &t[1..t.len() - 1];
            if name.is_empty() || p.is_empty() {
                return false;
            }
            continue;
        }
        if t != p {
            return false;
        }
    }
    true
}

/// Check if a route matches the relative path and method, returning `(matched, allowed)`.
///
/// 检查路由是否匹配相对路径与方法，返回 `(匹配, 允许)`。
fn route_match(mount: &HttpRouteMount, method: HttpMethod, relative_path: &str) -> (bool, bool) {
    if mount.routes.is_empty() {
        return (true, true);
    }
    let relative = normalize_path(relative_path);
    let mut matched_path = false;
    for route in &mount.routes {
        if is_root_route(&route.path) {
            continue;
        }
        let route_path = normalize_path(&route.path);
        if path_template_match(&route_path, &relative) {
            matched_path = true;
            if route.method == method {
                return (true, true);
            }
        }
    }
    (matched_path, false)
}

/// Match a root route (`//`) against the absolute path and method.
///
/// 将根路由 (`//`) 与绝对路径及方法匹配。
fn root_route_match(
    mount: &HttpRouteMount,
    method: HttpMethod,
    absolute_path: &str,
) -> (bool, bool, String) {
    let absolute = normalize_path(absolute_path);
    let mut matched_path = false;
    for route in &mount.routes {
        if !is_root_route(&route.path) {
            continue;
        }
        let route_path = normalize_root_route(&route.path);
        if route_path == absolute {
            matched_path = true;
            if route.method == method {
                return (true, true, route_path);
            }
        }
    }
    (matched_path, false, absolute)
}

/// A root route starts with `//` and is matched against the absolute path.
///
/// 根路由以 `//` 开头，并针对绝对路径匹配。
fn is_root_route(path: &str) -> bool {
    path.starts_with("//")
}

/// Normalize a root route by stripping its leading slashes.
///
/// 通过去除前导斜杠规范化根路由。
fn normalize_root_route(path: &str) -> String {
    normalize_path(path.trim_start_matches('/'))
}

/// Map an Axum HTTP method to the SDK `HttpMethod` enum.
///
/// 将 Axum HTTP 方法映射到 SDK `HttpMethod` 枚举。
fn to_sdk_method(method: &Method) -> Option<HttpMethod> {
    match *method {
        Method::GET => Some(HttpMethod::Get),
        Method::POST => Some(HttpMethod::Post),
        Method::PUT => Some(HttpMethod::Put),
        Method::PATCH => Some(HttpMethod::Patch),
        Method::DELETE => Some(HttpMethod::Delete),
        Method::OPTIONS => Some(HttpMethod::Options),
        _ => None,
    }
}

/// Parse a string effect hint into `ConfigEffect` (defaulting to `Immediate`).
///
/// 将字符串效果提示解析为 `ConfigEffect`（默认为 `Immediate`）。
fn parse_effect(effect: Option<&str>) -> ConfigEffect {
    match effect {
        Some("new_sessions") => ConfigEffect::NewSessionsOnly,
        Some("module_restart") => ConfigEffect::ModuleRestartRequired,
        Some("engine_restart") => ConfigEffect::EngineRestartRequired,
        _ => ConfigEffect::Immediate,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use axum::response::IntoResponse;
    use cheetah_sdk::{
        CancellationToken, ConfigApplyApi, ConfigApplyOutcome, ConfigEffect, ConfigProvider,
        ConfigRollbackToken, ConfigSchemaRegistry, ConfigValidator, HealthApi, HttpMethod,
        HttpRequest, HttpResponse, HttpRouteDescriptor, HttpRouteMount, MetricsApi,
        ModuleConfigApplyReport, ModuleConfigChange, ModuleHttpService, ModuleId, ModuleManagerApi,
        ModuleState, PublisherOptions, PublisherSink, RegisteredSchema, SdkError,
        ServiceDescriptor, ServiceRegistry, StreamKey, StreamManagerApi, StreamSnapshot,
        SubscriberOptions, SubscriberSource, TaskId, TaskKind, TaskOutcome, TaskSnapshot,
        TaskState, TaskSystemApi,
    };
    use serde_json::json;

    use super::{
        patch_global_config, path_template_match, relative_path, root_route_match, route_match,
        ControlState, PatchRequest,
    };

    struct DummyHttpService;

    #[async_trait]
    impl ModuleHttpService for DummyHttpService {
        async fn handle(&self, _req: HttpRequest) -> Result<HttpResponse, SdkError> {
            Ok(HttpResponse::ok_json("{}"))
        }
    }

    #[test]
    fn normalizes_relative_path_from_mount_prefix() {
        assert_eq!(relative_path("/noop", "/noop"), Some("/".to_string()));
        assert_eq!(relative_path("/noop", "/noop/"), Some("/".to_string()));
        assert_eq!(
            relative_path("/noop", "/noop/status"),
            Some("/status".to_string())
        );
        assert_eq!(relative_path("/noop", "/other/status"), None);
    }

    #[test]
    fn route_match_checks_path_and_method() {
        let mount = HttpRouteMount {
            module_id: cheetah_sdk::ModuleId::new("noop"),
            prefix: "/noop".to_string(),
            routes: vec![HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/status".to_string(),
            }],
            service: Arc::new(DummyHttpService),
        };

        assert_eq!(
            route_match(&mount, HttpMethod::Get, "/status"),
            (true, true)
        );
        assert_eq!(
            route_match(&mount, HttpMethod::Post, "/status"),
            (true, false)
        );
        assert_eq!(
            route_match(&mount, HttpMethod::Get, "/missing"),
            (false, false)
        );
    }

    #[test]
    fn path_template_matches_single_segment_parameters() {
        assert!(path_template_match(
            "/media/{vhost}/{app}/{stream}",
            "/media/__defaultVhost__/live/obs"
        ));
        assert!(path_template_match(
            "/media/{vhost}/{app}/{stream}/online",
            "/media/__defaultVhost__/live/obs/online"
        ));
        assert!(!path_template_match(
            "/media/{vhost}/{app}/{stream}",
            "/media/__defaultVhost__/live"
        ));
        assert!(!path_template_match(
            "/media/{vhost}/{app}/{stream}",
            "/media/__defaultVhost__/live/obs/extra"
        ));
        assert!(!path_template_match(
            "/media/{vhost}/{app}/{stream}",
            "/other/__defaultVhost__/live/obs"
        ));
    }

    #[test]
    fn route_match_allows_path_templates() {
        let mount = HttpRouteMount {
            module_id: cheetah_sdk::ModuleId::new("noop"),
            prefix: "/api/v1".to_string(),
            routes: vec![
                HttpRouteDescriptor {
                    method: HttpMethod::Get,
                    path: "/media/{vhost}/{app}/{stream}".to_string(),
                },
                HttpRouteDescriptor {
                    method: HttpMethod::Post,
                    path: "/media/{vhost}/{app}/{stream}/close".to_string(),
                },
            ],
            service: Arc::new(DummyHttpService),
        };

        assert_eq!(
            route_match(&mount, HttpMethod::Get, "/media/__defaultVhost__/live/obs"),
            (true, true)
        );
        assert_eq!(
            route_match(
                &mount,
                HttpMethod::Post,
                "/media/__defaultVhost__/live/obs/close"
            ),
            (true, true)
        );
        assert_eq!(
            route_match(&mount, HttpMethod::Post, "/media/__defaultVhost__/live/obs"),
            (true, false)
        );
        assert_eq!(
            route_match(
                &mount,
                HttpMethod::Get,
                "/media/__defaultVhost__/live/obs/close"
            ),
            (true, false)
        );
        assert_eq!(
            route_match(&mount, HttpMethod::Get, "/sessions/foo/kick"),
            (false, false)
        );
    }

    #[test]
    fn root_route_descriptor_does_not_match_under_module_prefix() {
        let mount = HttpRouteMount {
            module_id: cheetah_sdk::ModuleId::new("noop"),
            prefix: "/api/v1/noop".to_string(),
            routes: vec![HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "//rtc/v1/whep".to_string(),
            }],
            service: Arc::new(DummyHttpService),
        };

        assert_eq!(
            route_match(&mount, HttpMethod::Post, "/rtc/v1/whep"),
            (false, false)
        );
        assert_eq!(
            root_route_match(&mount, HttpMethod::Post, "/rtc/v1/whep"),
            (true, true, "/rtc/v1/whep".to_string())
        );
        assert_eq!(
            root_route_match(&mount, HttpMethod::Options, "/rtc/v1/whep"),
            (true, false, "/rtc/v1/whep".to_string())
        );
    }

    struct DummyHealth;
    impl HealthApi for DummyHealth {
        fn is_live(&self) -> bool {
            true
        }
        fn is_ready(&self) -> bool {
            true
        }
    }

    struct DummyMetrics;
    impl MetricsApi for DummyMetrics {
        fn render(&self) -> String {
            "ok".to_string()
        }
    }

    struct DummyStreams;
    #[async_trait]
    impl StreamManagerApi for DummyStreams {
        async fn open_publisher(
            &self,
            _stream_key: StreamKey,
            _options: PublisherOptions,
        ) -> Result<Box<dyn PublisherSink>, SdkError> {
            Err(SdkError::Unavailable("unused".to_string()))
        }

        async fn open_subscriber(
            &self,
            _stream_key: StreamKey,
            _options: SubscriberOptions,
        ) -> Result<Box<dyn SubscriberSource>, SdkError> {
            Err(SdkError::Unavailable("unused".to_string()))
        }

        async fn list_streams(&self) -> Result<Vec<StreamSnapshot>, SdkError> {
            Ok(Vec::new())
        }

        async fn get_stream(
            &self,
            _stream_key: &StreamKey,
        ) -> Result<Option<StreamSnapshot>, SdkError> {
            Ok(None)
        }

        async fn request_keyframe(&self, _stream_key: &StreamKey) -> Result<(), SdkError> {
            Ok(())
        }

        async fn close_idle_publishers(&self, _max_idle_secs: u64) -> Result<usize, SdkError> {
            Ok(0)
        }
    }

    struct DummyTasks;
    impl TaskSystemApi for DummyTasks {
        fn create_task(
            &self,
            _parent_id: Option<TaskId>,
            _kind: TaskKind,
            _owner: &str,
            _label: &str,
        ) -> Result<TaskId, SdkError> {
            Ok(TaskId(1))
        }

        fn cancel(&self, _task_id: TaskId, _reason: Option<&str>) -> Result<(), SdkError> {
            Ok(())
        }

        fn finish(&self, _task_id: TaskId, _outcome: TaskOutcome) -> Result<(), SdkError> {
            Ok(())
        }

        fn token(&self, _task_id: TaskId) -> Result<CancellationToken, SdkError> {
            Ok(CancellationToken::new())
        }

        fn snapshot(&self) -> Vec<TaskSnapshot> {
            vec![TaskSnapshot {
                id: TaskId(1),
                parent_id: None,
                kind: TaskKind::Task,
                state: TaskState::Running,
                terminal_outcome: None,
                owner: "test".to_string(),
                label: "test".to_string(),
                level: 0,
                child_ids: Vec::new(),
                started_unix_millis: 0,
                updated_unix_millis: 0,
                finished_unix_millis: None,
                cancel_reason: None,
                finish_message: None,
                spawn_site: "test:1:1".to_string(),
            }]
        }
    }

    struct DummyRegistry;
    impl ServiceRegistry for DummyRegistry {
        fn register(&self, _service: ServiceDescriptor) -> Result<(), SdkError> {
            Ok(())
        }
        fn get(&self, _name: &str) -> Option<ServiceDescriptor> {
            None
        }
        fn unregister(&self, _name: &str) -> Result<(), SdkError> {
            Ok(())
        }
        fn list_services(&self) -> Vec<ServiceDescriptor> {
            Vec::new()
        }
    }

    struct MockModules {
        fail_apply: bool,
    }

    #[async_trait]
    impl ModuleManagerApi for MockModules {
        fn modules(&self) -> Vec<(ModuleId, ModuleState)> {
            Vec::new()
        }

        fn http_mounts(&self) -> Vec<HttpRouteMount> {
            Vec::new()
        }

        async fn apply_module_config_change(
            &self,
            change: ModuleConfigChange,
        ) -> Result<ModuleConfigApplyReport, SdkError> {
            Ok(ModuleConfigApplyReport {
                module_id: change.module_id,
                effect: ConfigEffect::Immediate,
            })
        }

        async fn apply_module_config_changes(
            &self,
            changes: Vec<ModuleConfigChange>,
        ) -> Result<Vec<ModuleConfigApplyReport>, SdkError> {
            if self.fail_apply {
                return Err(SdkError::Internal(
                    "forced module apply failure".to_string(),
                ));
            }
            Ok(changes
                .into_iter()
                .map(|change| ModuleConfigApplyReport {
                    module_id: change.module_id,
                    effect: ConfigEffect::Immediate,
                })
                .collect())
        }

        async fn restart_module(&self, _module_id: &ModuleId) -> Result<(), SdkError> {
            Ok(())
        }

        async fn restart_modules(&self, _module_ids: Vec<ModuleId>) -> Result<(), SdkError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockConfigStore {
        rollback_called: Mutex<bool>,
    }

    impl ConfigProvider for MockConfigStore {
        fn global(&self) -> serde_json::Value {
            json!({"g": 1})
        }

        fn module(&self, _module_id: &ModuleId) -> serde_json::Value {
            json!({})
        }

        fn version(&self) -> u64 {
            1
        }
    }

    impl ConfigSchemaRegistry for MockConfigStore {
        fn register_global_schema(
            &self,
            _schema_name: &str,
            _default_value: serde_json::Value,
            _validator: Option<ConfigValidator>,
        ) -> Result<(), SdkError> {
            Ok(())
        }

        fn register_module_schema(
            &self,
            _module_id: ModuleId,
            _schema_name: &str,
            _default_value: serde_json::Value,
            _validator: Option<ConfigValidator>,
        ) -> Result<(), SdkError> {
            Ok(())
        }

        fn list_schemas(&self) -> Vec<RegisteredSchema> {
            Vec::new()
        }
    }

    impl ConfigApplyApi for MockConfigStore {
        fn apply_global_patch(
            &self,
            _patch: serde_json::Value,
            effect: ConfigEffect,
        ) -> Result<ConfigApplyOutcome, SdkError> {
            Ok(ConfigApplyOutcome {
                version: 2,
                effect,
                global_change: None,
                module_changes: vec![ModuleConfigChange {
                    module_id: ModuleId::new("m1"),
                    previous: json!({"v": 0}),
                    next: json!({"v": 1}),
                    previous_global: Some(json!({})),
                    next_global: Some(json!({})),
                }],
                rollback_token: Some(ConfigRollbackToken {
                    previous_global_runtime: Some(json!({"g": 1})),
                    previous_module_runtime: vec![(ModuleId::new("m1"), Some(json!({"v": 0})))],
                }),
            })
        }

        fn apply_module_patch(
            &self,
            _module_id: &ModuleId,
            _patch: serde_json::Value,
            effect: ConfigEffect,
        ) -> Result<ConfigApplyOutcome, SdkError> {
            Ok(ConfigApplyOutcome {
                version: 2,
                effect,
                global_change: None,
                module_changes: Vec::new(),
                rollback_token: None,
            })
        }

        fn rollback(&self, _token: ConfigRollbackToken) -> Result<(), SdkError> {
            *self.rollback_called.lock().expect("rollback lock") = true;
            Ok(())
        }
    }

    fn build_state(config: Arc<MockConfigStore>, fail_apply: bool) -> Arc<ControlState> {
        Arc::new(ControlState {
            health: Arc::new(DummyHealth),
            metrics: Arc::new(DummyMetrics),
            modules: Arc::new(MockModules { fail_apply }),
            streams: Arc::new(DummyStreams),
            tasks: Arc::new(DummyTasks),
            config: config.clone(),
            config_apply: config.clone(),
            config_schemas: config,
            service_registry: Arc::new(DummyRegistry),
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn patch_global_config_returns_apply_reports() {
        let config = Arc::new(MockConfigStore::default());
        let state = build_state(config, false);
        let response = patch_global_config(
            axum::extract::Extension(state),
            axum::Json(PatchRequest {
                patch: json!({"g": 2}),
                effect: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(
            json["module_apply_reports"].as_array().map(Vec::len),
            Some(1)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn patch_global_config_rolls_back_on_module_apply_failure() {
        let config = Arc::new(MockConfigStore::default());
        let state = build_state(config.clone(), true);
        let response = patch_global_config(
            axum::extract::Extension(state),
            axum::Json(PatchRequest {
                patch: json!({"g": 2}),
                effect: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert!(
            *config.rollback_called.lock().expect("rollback lock"),
            "rollback must be called when module apply fails"
        );
    }
}
