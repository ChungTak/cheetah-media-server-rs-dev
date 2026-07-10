/// `avcc_nal_length_size` function.
/// `avcc_nal_length_size` 函数.
pub(crate) fn avcc_nal_length_size(avcc: &[u8]) -> Option<usize> {
    if avcc.len() < 5 {
        return None;
    }
    Some(((avcc[4] & 0x03) + 1) as usize)
}

/// `hvcc_nal_length_size` function.
/// `hvcc_nal_length_size` 函数.
pub(crate) fn hvcc_nal_length_size(hvcc: &[u8]) -> Option<usize> {
    if hvcc.len() < 22 {
        return None;
    }
    Some(((hvcc[21] & 0x03) + 1) as usize)
}

/// `normalize_nal_length_size` function.
/// `normalize_nal_length_size` 函数.
pub(crate) fn normalize_nal_length_size(length_size: usize) -> usize {
    match length_size {
        1 | 2 | 4 => length_size,
        _ => 4,
    }
}
