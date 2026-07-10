use std::fmt;

use serde::{Deserialize, Serialize};

/// Generate a `u64` newtype identifier with `Display` and serde support.
///
/// Used for all domain identifiers that are numeric under the hood but should
/// not be confused with arbitrary integers in the type system.
///
/// 生成具有 `Display` 与 serde 支持的 `u64` newtype 标识符。
///
/// 用于所有底层为数字但不应在类型系统中与普通整数混淆的领域标识符。
macro_rules! id_u64 {
    ($name:ident) => {
        /// Numeric identifier.
        /// 数字标识符。
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_u64!(StreamId);
id_u64!(TaskId);
id_u64!(RoomId);
id_u64!(PublisherId);
id_u64!(SubscriberId);
id_u64!(SessionId);

/// Human-readable identifier for a module instance.
///
/// Module IDs are strings such as `rtmp` or `hls` and are used by the engine to
/// route configuration, events, and HTTP requests to the correct module.
///
/// 模块实例的人类可读标识符。
///
/// 模块 ID 是类似 `rtmp` 或 `hls` 的字符串，引擎用它将配置、事件和 HTTP 请求
/// 路由到正确的模块。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModuleId(pub String);

impl ModuleId {
    /// Create a module id from any string-like value.
    /// 从任意字符串值创建模块 ID。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Logical address of a stream inside the server.
///
/// Streams are addressed by `namespace/path` so that multiple applications or
/// tenants can share the same `cheetah` instance without collisions. A `StreamKey`
/// is used by publishers, subscribers, and control APIs to refer to the same stream.
///
/// 流在服务器内的逻辑地址。
///
/// 流通过 `namespace/path` 寻址，使多个应用或租户可以共享同一个 `cheetah`
/// 实例而不冲突。发布者、订阅者和控制 API 使用 `StreamKey` 指向同一个流。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StreamKey {
    /// Grouping scope for the stream, e.g. an application or tenant name.
    /// 流的分组作用域，例如应用或租户名称。
    pub namespace: String,
    /// Stream name within the namespace.
    /// 作用域内的流名称。
    pub path: String,
}

impl StreamKey {
    /// Create a stream key from namespace and path string-likes.
    /// 使用作用域和路径字符串创建流键。
    pub fn new(namespace: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            path: path.into(),
        }
    }
}

impl fmt::Display for StreamKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.namespace, self.path)
    }
}
