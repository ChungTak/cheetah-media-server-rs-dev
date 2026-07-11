/// Extracts the NAL unit length size (in bytes) from an H.264 AVCC configuration.
///
/// Returns `None` if the buffer is too short. The length size is stored in the
/// lower 2 bits of the 5th byte, so the value is `(avcc[4] & 0x03) + 1`.
///
/// 从 H.264 AVCC 配置中提取 NAL 单元长度大小（字节）。
///
/// 缓冲区过短时返回 `None`。长度大小保存在第 5 字节的低 2 位，
/// 因此值为 `(avcc[4] & 0x03) + 1`。
pub(crate) fn avcc_nal_length_size(avcc: &[u8]) -> Option<usize> {
    if avcc.len() < 5 {
        return None;
    }
    Some(((avcc[4] & 0x03) + 1) as usize)
}

/// Extracts the NAL unit length size (in bytes) from an H.265 HEVC configuration.
///
/// Returns `None` if the buffer is too short. The length size is stored in the
/// lower 2 bits of the 22nd byte, so the value is `(hvcc[21] & 0x03) + 1`.
///
/// 从 H.265/HEVC 配置中提取 NAL 单元长度大小（字节）。
///
/// 缓冲区过短时返回 `None`。长度大小保存在第 22 字节的低 2 位，
/// 因此值为 `(hvcc[21] & 0x03) + 1`。
pub(crate) fn hvcc_nal_length_size(hvcc: &[u8]) -> Option<usize> {
    if hvcc.len() < 22 {
        return None;
    }
    Some(((hvcc[21] & 0x03) + 1) as usize)
}

/// Normalizes a NAL length size to a supported value (1, 2, or 4 bytes).
///
/// Any unsupported value is clamped to 4 bytes, which is the most common default.
///
/// 将 NAL 长度大小归一化为支持的值（1、2 或 4 字节）。
///
/// 不支持的值统一钳制为 4 字节，这是最常用的默认值。
pub(crate) fn normalize_nal_length_size(length_size: usize) -> usize {
    match length_size {
        1 | 2 | 4 => length_size,
        _ => 4,
    }
}
