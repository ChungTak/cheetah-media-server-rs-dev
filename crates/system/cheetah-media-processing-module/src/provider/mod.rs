//! Media processing providers.

#[cfg(any(
    feature = "media-processing-image",
    feature = "media-processing-audio",
    feature = "media-processing-video",
))]
pub(crate) mod avcodec_registry;

#[cfg(feature = "media-processing-audio")]
pub mod audio;

#[cfg(feature = "media-processing-image")]
pub mod image;

#[cfg(feature = "media-processing-video")]
pub mod video;

#[cfg(feature = "media-processing-image")]
mod semaphore;

#[cfg(feature = "media-processing-image")]
pub use image::ImageProcessProvider;

#[cfg(feature = "media-processing-caption")]
pub mod caption;

#[cfg(feature = "media-processing-cpu")]
pub mod transcode;

#[cfg(feature = "media-processing-cpu")]
pub mod abr;

#[cfg(feature = "media-processing-cpu")]
pub mod mix;

#[cfg(feature = "media-processing-cpu")]
pub mod mosaic;

#[cfg(feature = "media-processing-cpu")]
pub(crate) mod mixer;

#[cfg(feature = "media-processing-cpu")]
pub(crate) mod mosaicker;

#[cfg(feature = "media-processing-cpu")]
pub(crate) mod mosaic_canvas;

#[cfg(all(test, feature = "media-processing-cpu"))]
mod mosaicker_tests;

#[cfg(feature = "media-processing-caption")]
pub use caption::MediaProcessingProvider;
