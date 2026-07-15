/// Authorization scope for control-plane operations.
///
/// 控制面操作的授权 scope。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MediaScope {
    MediaRead,
    MediaControl,
    MediaPublish,
    MediaConsume,
    RecordManage,
    FileRead,
    FileDelete,
    ServerAdmin,
}

impl MediaScope {
    /// Human-readable identifier used in errors and configuration.
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaScope::MediaRead => "media.read",
            MediaScope::MediaControl => "media.control",
            MediaScope::MediaPublish => "media.publish",
            MediaScope::MediaConsume => "media.consume",
            MediaScope::RecordManage => "record.manage",
            MediaScope::FileRead => "file.read",
            MediaScope::FileDelete => "file.delete",
            MediaScope::ServerAdmin => "server.admin",
        }
    }
}

impl std::fmt::Display for MediaScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MediaScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "media.read" => Ok(MediaScope::MediaRead),
            "media.control" => Ok(MediaScope::MediaControl),
            "media.publish" => Ok(MediaScope::MediaPublish),
            "media.consume" => Ok(MediaScope::MediaConsume),
            "record.manage" => Ok(MediaScope::RecordManage),
            "file.read" => Ok(MediaScope::FileRead),
            "file.delete" => Ok(MediaScope::FileDelete),
            "server.admin" => Ok(MediaScope::ServerAdmin),
            _ => Err(format!("unknown scope: {s}")),
        }
    }
}

impl serde::Serialize for MediaScope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for MediaScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse::<MediaScope>().map_err(serde::de::Error::custom)
    }
}

/// Authenticated principal returned by a `ControlAuthApi` implementation.
///
/// `ControlAuthApi` 实现返回的已认证 principal。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub identity: String,
    pub scopes: Vec<MediaScope>,
}

impl Principal {
    pub fn anonymous() -> Self {
        Self {
            identity: "anonymous".to_string(),
            scopes: Vec::new(),
        }
    }

    pub fn has_scope(&self, scope: &MediaScope) -> bool {
        self.scopes
            .iter()
            .any(|s| s == scope || *s == MediaScope::ServerAdmin)
    }
}

/// Raw credentials extracted from an incoming HTTP request.
///
/// 从传入 HTTP 请求中提取的原始凭证。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthCredentials {
    pub authorization_header: Option<String>,
    pub mtls_identity: Option<String>,
    pub deployment_token: Option<String>,
}
