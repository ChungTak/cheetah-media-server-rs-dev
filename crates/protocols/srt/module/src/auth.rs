/// Authorization context built from the parsed stream id.
///
/// 从解析后的 stream id 构建的鉴权上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SrtAuthContext {
    pub mode: SrtStreamMode,
    pub vhost: String,
    pub app: String,
    pub stream: String,
    pub stream_key: StreamKey,
    pub user: Option<String>,
    pub auth_params: BTreeMap<String, String>,
    pub peer_addr: Option<SocketAddr>,
}

impl SrtAuthContext {
    /// Encode auth_params as a `k=v&...` query string for webhook hooks.
    ///
    /// 将 auth_params 编码为 `k=v&...` 查询字符串，供 webhook 使用。
    #[allow(dead_code)]
    pub fn auth_params_as_query(&self) -> String {
        let mut parts = Vec::with_capacity(self.auth_params.len());
        for (key, value) in &self.auth_params {
            parts.push(format!("{key}={value}"));
        }
        parts.join("&")
    }
}
/// Validate the stream against global or per-user auth tokens.
///
/// 使用全局或每个用户的 token 校验流。
fn authorize_stream(config: &SrtModuleConfig, auth: &SrtAuthContext) -> Result<(), String> {
    if !config.auth.enabled {
        return Ok(());
    }

    let token = auth.auth_params.get("token");
    let global_token = match auth.mode {
        SrtStreamMode::Publish => &config.auth.publish_token,
        SrtStreamMode::Request | SrtStreamMode::Play => &config.auth.request_token,
    };
    if !global_token.is_empty() && token.is_some_and(|value| value == global_token) {
        return Ok(());
    }

    if let (Some(user), Some(token)) = (auth.user.as_deref(), token) {
        if config
            .auth
            .users
            .iter()
            .any(|entry| entry.username == user && entry.token == *token)
        {
            return Ok(());
        }
    }

    Err("reject:auth_rejected".to_string())
}
