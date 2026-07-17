//! Registry construction for `avcodec-rs` backends used across image/audio/video
//! processing providers.

use cheetah_media_api::{error::Result, MediaError};

use crate::config::MediaProcessingModuleConfig;

/// Builds an `avcodec-rs` registry limited to the configured profile.
///
/// 根据配置的 profile 构建 `avcodec-rs` 注册表。
pub(crate) fn build_registry(
    config: &MediaProcessingModuleConfig,
) -> Result<avcodec::core::Registry> {
    match config.profile.as_str() {
        "native-free" => Ok(filter_native_free_registry()),
        "software" if cfg!(feature = "avcodec-profile-software") => {
            Ok(avcodec::default_registry_builder().build())
        }
        "software" => Err(MediaError::unsupported(
            "software profile requires the avcodec-profile-software feature",
        )),
        _ => Err(MediaError::invalid_argument(format!(
            "unsupported avcodec profile: {}",
            config.profile
        ))),
    }
}

/// Builds a `Registry` from the default avcodec backend set but restricted to
/// audited native-free software backend ids.
///
/// `native-free` means no vendor-specific hardware SDKs; pure-software and
/// audited C libraries (e.g. `libyuv`, `fdk-aac`, `opus`) are included.
fn filter_native_free_registry() -> avcodec::core::Registry {
    const ALLOWED: &[&str] = &[
        // image
        "jpeg",
        "zune",
        "libyuv",
        // video
        "rust-h264",
        "rust-h265",
        // audio
        "g711",
        "opus",
        "fdk-aac",
    ];

    let all = avcodec::default_registry_builder();
    let mut filtered = avcodec::core::RegistryBuilder::new();
    for backend in all.backends() {
        if ALLOWED.contains(&backend.id()) {
            filtered = filtered.with_backend(*backend);
        }
    }
    filtered.build()
}
