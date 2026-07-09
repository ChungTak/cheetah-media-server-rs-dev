use alloc::string::String;
use core::panic::Location;

/// 错误的种类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// 输入参数的格式或结构无效
    InvalidInput,

    /// 输入数据无效或已损坏
    InvalidData,

    /// 结构体等内部状态不合法，或无法执行所请求的操作
    InvalidState,

    /// 不支持的操作或数据格式
    Unsupported,

    // 以下是内部类型，不在文档中显示
    #[doc(hidden)]
    InsufficientBuffer,
}

/// 错误类型
pub struct Error {
    /// 发生的错误种类
    pub kind: ErrorKind,

    /// 错误发生的原因
    pub reason: String,

    /// 错误创建时的源代码位置
    pub location: &'static Location<'static>,
}

impl Error {
    /// 创建 [`Error`] 实例
    #[track_caller]
    pub fn new(kind: ErrorKind) -> Self {
        Self::with_reason(kind, String::new())
    }

    /// 创建带错误原因的 [`Error`] 实例
    #[track_caller]
    pub fn with_reason<T: Into<String>>(kind: ErrorKind, reason: T) -> Self {
        Self {
            kind,
            reason: reason.into(),
            location: Location::caller(),
        }
    }

    #[track_caller]
    pub(crate) fn invalid_data<T: Into<String>>(reason: T) -> Self {
        Self::with_reason(ErrorKind::InvalidData, reason)
    }

    #[track_caller]
    pub(crate) fn invalid_input<T: Into<String>>(reason: T) -> Self {
        Self::with_reason(ErrorKind::InvalidInput, reason)
    }

    #[track_caller]
    pub(crate) fn invalid_state<T: Into<String>>(reason: T) -> Self {
        Self::with_reason(ErrorKind::InvalidState, reason)
    }

    #[track_caller]
    pub(crate) fn unsupported<T: Into<String>>(reason: T) -> Self {
        Self::with_reason(ErrorKind::Unsupported, reason)
    }

    #[track_caller]
    pub(crate) fn insufficient_buffer() -> Self {
        Self::new(ErrorKind::InsufficientBuffer)
    }

    #[track_caller]
    pub(crate) fn check_buffer_size(required_size: usize, buf: &[u8]) -> Result<(), Self> {
        if buf.len() < required_size {
            Err(Self::insufficient_buffer())
        } else {
            Ok(())
        }
    }
}

impl core::fmt::Debug for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self}")
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.reason)?;
        write!(f, " (at {}:{})", self.location.file(), self.location.line())?;
        Ok(())
    }
}
