use alloc::string::String;
use core::panic::Location;

/// 错误的种类
/// The kind of error produced by the protocol core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Invalid input parameter format or structure.
    /// 输入参数的格式或结构无效。
    InvalidInput,

    /// Invalid or corrupted input data.
    /// 输入数据无效或已损坏。
    InvalidData,

    /// Illegal internal state, or the requested operation cannot be performed.
    /// 结构体等内部状态不合法，或无法执行所请求的操作。
    InvalidState,

    /// Unsupported operation or data format.
    /// 不支持的操作或数据格式。
    Unsupported,

    // 以下是内部类型，不在文档中显示
    /// The input buffer is too short to complete decoding.
    /// 输入缓冲区不足以完成解码。
    #[doc(hidden)]
    InsufficientBuffer,
}

/// 错误类型
/// The concrete error type used throughout the protocol core.
pub struct Error {
    /// The kind of error.
    /// 发生的错误种类。
    pub kind: ErrorKind,

    /// Human-readable reason for the error.
    /// 错误发生的原因。
    pub reason: String,

    /// Source location where the error was created.
    /// 错误创建时的源代码位置。
    pub location: &'static Location<'static>,
}

impl Error {
    /// 创建 [`Error`] 实例
    /// Creates an `Error` with the given kind and no additional reason.
    #[track_caller]
    pub fn new(kind: ErrorKind) -> Self {
        Self::with_reason(kind, String::new())
    }

    /// 创建带错误原因的 [`Error`] 实例
    /// Creates an `Error` with the given kind and a descriptive reason.
    #[track_caller]
    pub fn with_reason<T: Into<String>>(kind: ErrorKind, reason: T) -> Self {
        Self {
            kind,
            reason: reason.into(),
            location: Location::caller(),
        }
    }

    /// Convenience constructor for an `InvalidData` error.
    /// 创建 InvalidData 错误的便捷方法。
    #[track_caller]
    pub(crate) fn invalid_data<T: Into<String>>(reason: T) -> Self {
        Self::with_reason(ErrorKind::InvalidData, reason)
    }

    /// Convenience constructor for an `InvalidInput` error.
    /// 创建 InvalidInput 错误的便捷方法。
    #[track_caller]
    pub(crate) fn invalid_input<T: Into<String>>(reason: T) -> Self {
        Self::with_reason(ErrorKind::InvalidInput, reason)
    }

    /// Convenience constructor for an `InvalidState` error.
    /// 创建 InvalidState 错误的便捷方法。
    #[track_caller]
    pub(crate) fn invalid_state<T: Into<String>>(reason: T) -> Self {
        Self::with_reason(ErrorKind::InvalidState, reason)
    }

    /// Convenience constructor for an `Unsupported` error.
    /// 创建 Unsupported 错误的便捷方法。
    #[track_caller]
    pub(crate) fn unsupported<T: Into<String>>(reason: T) -> Self {
        Self::with_reason(ErrorKind::Unsupported, reason)
    }

    /// Convenience constructor for an `InsufficientBuffer` error.
    /// 创建 InsufficientBuffer 错误的便捷方法。
    #[track_caller]
    pub(crate) fn insufficient_buffer() -> Self {
        Self::new(ErrorKind::InsufficientBuffer)
    }

    /// Returns `InsufficientBuffer` if `buf` has fewer than `required_size` bytes.
    /// 检查缓冲区大小是否足够。
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
