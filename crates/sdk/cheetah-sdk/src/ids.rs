use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! id_u64 {
    ($name:ident) => {
        #[doc = concat!("Numeric identifier for `", stringify!($name), "`.

`", stringify!($name), "` 的 u64 数字标识。")]
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

/// Unique identifier for a module. It is a string (not a numeric id) because modules
/// are named by their crate/role.
///
/// module 唯一标识。使用字符串而非数字 id，因为 module 按 crate/角色命名。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModuleId(pub String);

impl ModuleId {
    /// Create a new module id from a string.
    ///
    /// 从字符串创建 module id。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Logical stream key composed of a namespace and a path.
///
/// `StreamKey` is the primary addressing key used by publishers and subscribers.
///
/// 由 namespace 和 path 组成的逻辑流键。
///
/// `StreamKey` 是发布者和订阅者使用的主要寻址键。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StreamKey {
    pub namespace: String,
    pub path: String,
}

impl StreamKey {
    /// Build a stream key from a namespace and a path.
    ///
    /// 从 namespace 和 path 构建 stream key。
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
