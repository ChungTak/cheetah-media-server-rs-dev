use crate::error::{SrtCoreError, SrtCoreResult};

/// SRT version 1.3.0 encoded as `(major << 16) | (minor << 8) | patch`.
///
/// SRT 版本 1.3.0 的编码值。
pub const SRT_VERSION_1_3_0: u32 = 0x0001_0300;

/// SRT version 1.5.0 encoded as `(major << 16) | (minor << 8) | patch`.
///
/// SRT 版本 1.5.0 的编码值。
pub const SRT_VERSION_1_5_0: u32 = 0x0001_0500;

/// Parse a dotted SRT version string (e.g. `"1.3.0"`) into a 32-bit version code.
///
/// 将点分 SRT 版本字符串（如 `"1.3.0"`）解析为 32 位版本码。
pub fn parse_srt_version(s: &str) -> SrtCoreResult<u32> {
    let parts: Vec<&str> = s.trim().split('.').collect();
    if parts.len() != 3 {
        return Err(SrtCoreError::InvalidConfig(format!(
            "SRT version must be `major.minor.patch`: `{s}`"
        )));
    }
    let major = parse_version_component(parts[0], "major", s)?;
    let minor = parse_version_component(parts[1], "minor", s)?;
    let patch = parse_version_component(parts[2], "patch", s)?;
    if major > 255 || minor > 255 || patch > 255 {
        return Err(SrtCoreError::InvalidConfig(format!(
            "SRT version components must be 0-255: `{s}`"
        )));
    }
    Ok((major << 16) | (minor << 8) | patch)
}

fn parse_version_component(value: &str, name: &str, original: &str) -> SrtCoreResult<u32> {
    value.parse::<u32>().map_err(|err| {
        SrtCoreError::InvalidConfig(format!(
            "invalid {name} component in SRT version `{original}`: {err}"
        ))
    })
}

/// Format a 32-bit SRT version code back to a dotted string.
///
/// 将 32 位 SRT 版本码格式化为点分字符串。
pub fn format_srt_version(v: u32) -> String {
    let major = (v >> 16) & 0xff;
    let minor = (v >> 8) & 0xff;
    let patch = v & 0xff;
    format!("{major}.{minor}.{patch}")
}

/// Check whether `peer` is at least `min`.
///
/// 检查 `peer` 是否大于或等于 `min`。
pub fn version_at_least(peer: u32, min: u32) -> bool {
    peer >= min
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_1_3_0() {
        assert_eq!(parse_srt_version("1.3.0").unwrap(), SRT_VERSION_1_3_0);
    }

    #[test]
    fn parse_1_5_0() {
        assert_eq!(parse_srt_version("1.5.0").unwrap(), SRT_VERSION_1_5_0);
    }

    #[test]
    fn roundtrip() {
        assert_eq!(format_srt_version(SRT_VERSION_1_3_0), "1.3.0");
        assert_eq!(format_srt_version(SRT_VERSION_1_5_0), "1.5.0");
        assert_eq!(format_srt_version(0x0001_0209), "1.2.9");
    }

    #[test]
    fn comparison() {
        assert!(!version_at_least(0x0001_0209, SRT_VERSION_1_3_0));
        assert!(version_at_least(SRT_VERSION_1_3_0, SRT_VERSION_1_3_0));
        assert!(version_at_least(SRT_VERSION_1_5_0, SRT_VERSION_1_3_0));
    }

    #[test]
    fn invalid_rejected() {
        assert!(parse_srt_version("1.3").is_err());
        assert!(parse_srt_version("1.3.0.0").is_err());
        assert!(parse_srt_version("1.a.0").is_err());
        assert!(parse_srt_version("").is_err());
    }

    #[test]
    fn overflow_component_rejected() {
        assert!(parse_srt_version("256.0.0").is_err());
        assert!(parse_srt_version("0.256.0").is_err());
        assert!(parse_srt_version("0.0.256").is_err());
        assert!(parse_srt_version("65536.0.0").is_err());
    }
}
