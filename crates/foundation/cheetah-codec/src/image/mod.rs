//! Lightweight image metadata parsing for content validation.
//!
//! 用于内容校验的轻量图片元数据解析。

pub mod jpeg;
pub mod png;

/// Width and height decoded from an image header.
///
/// 从图片头部解析出的宽高。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}
