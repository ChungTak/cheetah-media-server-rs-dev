//! Media processing providers.

#[cfg(any(
    feature = "media-processing-image",
    feature = "media-processing-audio",
    feature = "media-processing-video"
))]
pub(crate) mod avcodec_registry;

#[cfg(feature = "media-processing-audio")]
pub mod audio;

#[cfg(feature = "media-processing-image")]
pub mod image;

#[cfg(any(
    feature = "media-processing-image",
    feature = "media-processing-audio",
    feature = "media-processing-video"
))]
mod semaphore;

#[cfg(feature = "media-processing-image")]
pub use image::ImageProcessProvider;
