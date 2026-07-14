use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::MediaError;

macro_rules! id_wrapper {
    ($name:ident, $inner:ty) => {
        #[doc = concat!("Identifier for `", stringify!($name), "`.")]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $inner);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_wrapper!(SessionId, String);
id_wrapper!(RecordTaskId, String);
id_wrapper!(RecordFileId, String);
id_wrapper!(SnapshotId, String);
id_wrapper!(ProxyId, String);
id_wrapper!(RtpSessionId, String);
id_wrapper!(WebRtcRoomId, String);
id_wrapper!(FileHandle, String);
id_wrapper!(IdempotencyKey, String);
id_wrapper!(RequestId, String);

/// Default `vhost` value used when none is supplied.
///
/// 未提供 vhost 时的默认值。
pub const DEFAULT_VHOST: &str = "__defaultVhost__";

/// Validated virtual host name.
///
/// 经验证的虚拟主机名。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VhostName(pub String);

impl VhostName {
    /// Create a vhost name, returning an error if it is empty or contains invalid characters.
    ///
    /// 创建 vhost 名称；若为空或包含非法字符则返回错误。
    pub fn new(value: impl Into<String>) -> Result<Self, MediaError> {
        let value = value.into();
        if value.is_empty() {
            return Err(MediaError::invalid_argument("vhost must be non-empty"));
        }
        if value.contains('/') || value.contains('\\') {
            return Err(MediaError::invalid_argument(
                "vhost must not contain path separators",
            ));
        }
        Ok(Self(value))
    }

    /// Default virtual host name.
    ///
    /// 默认虚拟主机名。
    pub fn default_value() -> Self {
        Self(DEFAULT_VHOST.to_string())
    }
}

impl Default for VhostName {
    fn default() -> Self {
        Self::default_value()
    }
}

impl fmt::Display for VhostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Validated application name.
///
/// 经验证的应用名。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AppName(pub String);

impl AppName {
    /// Create an app name, returning an error if empty or invalid.
    ///
    /// 创建 app 名称；若为空或无效则返回错误。
    pub fn new(value: impl Into<String>) -> Result<Self, MediaError> {
        let value = value.into();
        if value.is_empty() {
            return Err(MediaError::invalid_argument("app must be non-empty"));
        }
        if value.contains('/') || value.contains('\\') {
            return Err(MediaError::invalid_argument(
                "app must not contain path separators",
            ));
        }
        Ok(Self(value))
    }
}

impl fmt::Display for AppName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Validated stream name.
///
/// 经验证的流名。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StreamName(pub String);

impl StreamName {
    /// Create a stream name, returning an error if empty or invalid.
    ///
    /// 创建 stream 名称；若为空或无效则返回错误。
    pub fn new(value: impl Into<String>) -> Result<Self, MediaError> {
        let value = value.into();
        if value.is_empty() {
            return Err(MediaError::invalid_argument("stream must be non-empty"));
        }
        if value.contains('/') || value.contains('\\') {
            return Err(MediaError::invalid_argument(
                "stream must not contain path separators",
            ));
        }
        Ok(Self(value))
    }
}

impl fmt::Display for StreamName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Output schema used to access a media resource.
///
/// 用于访问媒体资源的输出视图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum MediaSchema {
    Rtsp,
    Rtmp,
    HttpFlv,
    Hls,
    Webrtc,
    Ts,
    Fmp4,
    Srt,
    Rtp,
}

impl MediaSchema {
    /// Parse a schema from a string, returning `Unknown` if not recognized.
    ///
    /// 从字符串解析 schema；无法识别时返回 `Unknown`。
    pub fn parse(s: &str) -> Result<Self, MediaError> {
        match s.to_ascii_lowercase().as_str() {
            "rtsp" => Ok(MediaSchema::Rtsp),
            "rtmp" => Ok(MediaSchema::Rtmp),
            "http-flv" | "http_flv" | "flv" => Ok(MediaSchema::HttpFlv),
            "hls" => Ok(MediaSchema::Hls),
            "webrtc" | "rtc" => Ok(MediaSchema::Webrtc),
            "ts" => Ok(MediaSchema::Ts),
            "fmp4" => Ok(MediaSchema::Fmp4),
            "srt" => Ok(MediaSchema::Srt),
            "rtp" => Ok(MediaSchema::Rtp),
            other => Err(MediaError::invalid_argument(format!(
                "unknown schema: {other}"
            ))),
        }
    }
}

impl fmt::Display for MediaSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            MediaSchema::Rtsp => "rtsp",
            MediaSchema::Rtmp => "rtmp",
            MediaSchema::HttpFlv => "http-flv",
            MediaSchema::Hls => "hls",
            MediaSchema::Webrtc => "webrtc",
            MediaSchema::Ts => "ts",
            MediaSchema::Fmp4 => "fmp4",
            MediaSchema::Srt => "srt",
            MediaSchema::Rtp => "rtp",
        };
        write!(f, "{s}")
    }
}

/// Logical media key composed of vhost, app, stream, and optional schema.
///
/// `MediaKey` is the primary public addressing key for the media-domain API.
/// Multiple output schemas for the same vhost/app/stream represent views of
/// the same underlying media resource.
///
/// 媒体领域 API 的主寻址键，由 vhost、app、stream 与可选 schema 组成。
/// 同一 vhost/app/stream 的多个输出 schema 对应同一底层媒体资源的不同视图。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MediaKey {
    pub vhost: VhostName,
    pub app: AppName,
    pub stream: StreamName,
    pub schema: Option<MediaSchema>,
}

impl MediaKey {
    /// Build a media key, validating all components.
    ///
    /// 构建 media key，并校验所有组件。
    pub fn new(
        vhost: impl Into<String>,
        app: impl Into<String>,
        stream: impl Into<String>,
        schema: Option<MediaSchema>,
    ) -> Result<Self, MediaError> {
        Ok(Self {
            vhost: VhostName::new(vhost)?,
            app: AppName::new(app)?,
            stream: StreamName::new(stream)?,
            schema,
        })
    }

    /// Build a media key using the default vhost.
    ///
    /// 使用默认 vhost 构建 media key。
    pub fn with_default_vhost(
        app: impl Into<String>,
        stream: impl Into<String>,
        schema: Option<MediaSchema>,
    ) -> Result<Self, MediaError> {
        Self::new(VhostName::default_value().0, app, stream, schema)
    }

    /// Remove the schema, returning the canonical media key for resource lookup.
    ///
    /// 移除 schema，返回资源查找用的规范 media key。
    pub fn without_schema(&self) -> Self {
        Self {
            vhost: self.vhost.clone(),
            app: self.app.clone(),
            stream: self.stream.clone(),
            schema: None,
        }
    }

    /// Convert to a string representation that can be parsed back.
    ///
    /// 转换为可解析回来的字符串表示。
    pub fn to_canonical(&self) -> String {
        match &self.schema {
            Some(schema) => format!(
                "{}/{}/{}?schema={}",
                self.vhost, self.app, self.stream, schema
            ),
            None => format!("{}/{}/{}", self.vhost, self.app, self.stream),
        }
    }
}

impl fmt::Display for MediaKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_canonical())
    }
}

/// Bridge to legacy `StreamKey { namespace, path }`.
///
/// The default mapping is `namespace = app`, `path = stream`. When a non-default
/// vhost is present, the namespace is encoded as `vhost#app` so the mapping is
/// reversible. This helper is exposed by the domain crate so all adapters share
/// the same encoding.
///
/// 与旧 `StreamKey { namespace, path }` 的桥接。
///
/// 默认映射为 `namespace = app`、`path = stream`。当存在非默认 vhost 时，
/// namespace 编码为 `vhost#app` 以保证可逆性。此 helper 由 domain crate 暴露，
/// 使所有 adapter 共享同一编码。
pub struct StreamKeyBridge;

impl StreamKeyBridge {
    /// Encode a `MediaKey` into a `(namespace, path)` pair for legacy stream keys.
    ///
    /// 将 `MediaKey` 编码为旧 stream key 的 `(namespace, path)` 对。
    pub fn to_namespace_path(key: &MediaKey) -> (String, String) {
        let namespace = if key.vhost.0 == DEFAULT_VHOST {
            key.app.0.clone()
        } else {
            format!("{}#{}", key.vhost.0, key.app.0)
        };
        (namespace, key.stream.0.clone())
    }

    /// Decode a `(namespace, path)` pair into a `MediaKey`.
    ///
    /// 将 `(namespace, path)` 对解码为 `MediaKey`。
    pub fn from_namespace_path(namespace: &str, path: &str) -> Result<MediaKey, MediaError> {
        let parts: Vec<&str> = namespace.splitn(2, '#').collect();
        if parts.len() == 2 {
            MediaKey::new(parts[0], parts[1], path, None)
        } else {
            MediaKey::with_default_vhost(namespace, path, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_vhost_bridge_is_reversible() {
        let key = MediaKey::with_default_vhost("live", "test", None).unwrap();
        let (ns, path) = StreamKeyBridge::to_namespace_path(&key);
        assert_eq!(ns, "live");
        assert_eq!(path, "test");
        let decoded = StreamKeyBridge::from_namespace_path(&ns, &path).unwrap();
        assert_eq!(decoded, key);
    }

    #[test]
    fn non_default_vhost_bridge_is_reversible() {
        let key = MediaKey::new("custom", "live", "test", None).unwrap();
        let (ns, path) = StreamKeyBridge::to_namespace_path(&key);
        assert_eq!(ns, "custom#live");
        assert_eq!(path, "test");
        let decoded = StreamKeyBridge::from_namespace_path(&ns, &path).unwrap();
        assert_eq!(decoded, key);
    }

    #[test]
    fn empty_app_rejected() {
        assert!(MediaKey::with_default_vhost("", "test", None).is_err());
    }

    #[test]
    fn schema_parsing_and_display() {
        assert_eq!(
            MediaSchema::parse("HTTP-FLV").unwrap(),
            MediaSchema::HttpFlv
        );
        assert_eq!(MediaSchema::parse("webrtc").unwrap(), MediaSchema::Webrtc);
        assert!(MediaSchema::parse("unknown").is_err());
        assert_eq!(MediaSchema::Hls.to_string(), "hls");
    }
}
