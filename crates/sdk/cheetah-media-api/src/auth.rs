use crate::ids::MediaKey;
use serde::{Deserialize, Serialize};

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

/// Pattern for a single segment of a resource selector.
///
/// 资源选择器中单一段落的模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    Exact(String),
    Wildcard,
}

impl Pattern {
    pub fn parse(s: &str) -> Self {
        if s == "*" {
            Self::Wildcard
        } else {
            Self::Exact(s.to_string())
        }
    }

    pub fn matches(&self, value: &str) -> bool {
        match self {
            Self::Wildcard => true,
            Self::Exact(s) => s == value,
        }
    }
}

impl std::fmt::Display for Pattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wildcard => f.write_str("*"),
            Self::Exact(s) => f.write_str(s),
        }
    }
}

impl std::str::FromStr for Pattern {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(s))
    }
}

impl serde::Serialize for Pattern {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for Pattern {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse::<Pattern>().map_err(serde::de::Error::custom)
    }
}

/// Resource selector for per-tenant media grants.
///
/// 按租户媒体授权的资源选择器。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaResourceSelector {
    #[serde(default = "wildcard_pattern")]
    pub vhost: Pattern,
    #[serde(default = "wildcard_pattern")]
    pub app: Pattern,
    #[serde(default = "wildcard_pattern")]
    pub stream: Pattern,
}

impl MediaResourceSelector {
    pub fn matches(&self, key: &MediaKey) -> bool {
        self.vhost.matches(&key.vhost.0)
            && self.app.matches(&key.app.0)
            && self.stream.matches(&key.stream.0)
    }
}

fn wildcard_pattern() -> Pattern {
    Pattern::Wildcard
}

/// A resource-level grant that scopes a `MediaScope` to a vhost/app/stream
/// selector.
///
/// 将 `MediaScope` 限定到 vhost/app/stream 选择器的资源级授权。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaResourceGrant {
    pub selector: MediaResourceSelector,
    pub scopes: Vec<MediaScope>,
}

/// Authenticated principal returned by a `ControlAuthApi` implementation.
///
/// `ControlAuthApi` 实现返回的已认证 principal。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub identity: String,
    pub scopes: Vec<MediaScope>,
    pub resource_grants: Vec<MediaResourceGrant>,
}

impl Principal {
    pub fn anonymous() -> Self {
        Self {
            identity: "anonymous".to_string(),
            scopes: vec![MediaScope::MediaRead],
            resource_grants: Vec::new(),
        }
    }

    pub fn has_scope(&self, scope: &MediaScope) -> bool {
        self.scopes
            .iter()
            .any(|s| s == scope || *s == MediaScope::ServerAdmin)
    }

    /// Returns true if the scope is held globally or in any resource grant.
    ///
    /// This is intended for HTTP middleware gating: it does not check whether
    /// the eventual request key matches a grant; that finer check happens in the
    /// provider layer.
    pub fn has_scope_or_grant(&self, scope: &MediaScope) -> bool {
        if self.has_scope(scope) {
            return true;
        }
        self.resource_grants
            .iter()
            .any(|g| g.scopes.contains(scope))
    }

    /// Returns true if the principal has the requested scope, either globally
    /// or through a resource grant matching `key`.
    ///
    /// Operations without a media key can only use global scopes.
    pub fn authorizes(&self, scope: &MediaScope, key: Option<&MediaKey>) -> bool {
        if self.has_scope(scope) {
            return true;
        }
        let Some(key) = key else {
            return false;
        };
        self.resource_grants
            .iter()
            .any(|grant| grant.scopes.contains(scope) && grant.selector.matches(key))
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
