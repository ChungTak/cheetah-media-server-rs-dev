/// Typed identifiers used across the SDK and engine.
///
/// 在 SDK 和引擎中使用的类型化标识符。
use std::fmt;

use serde::{Deserialize, Serialize};

/// Macro defining a newtyped u64 identifier with serde and display support.
///
/// 定义具有 serde 和 display 支持的 u64 newtype 标识符。
macro_rules! id_u64 {
    ($name:ident) => {
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

/// String identifier for a module (used in manifests and HTTP routes).
///
/// 模块的字符串标识符（用于清单和 HTTP 路由）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModuleId(pub String);

impl ModuleId {
    /// Create a module id from any string-like value.
    ///
    /// 从任何类字符串值创建模块 id。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Logical address of a stream composed of a namespace and a path.
///
/// Streams are routed by `namespace/path`.
///
/// 由命名空间和路径组成的流逻辑地址。
///
/// 流按 `namespace/path` 路由。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StreamKey {
    pub namespace: String,
    pub path: String,
}

impl StreamKey {
    /// Build a stream key from namespace and path components.
    ///
    /// 从命名空间和路径组件构建流键。
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
