//! Media processing providers.

#[cfg(feature = "media-processing-image")]
pub mod image;

#[cfg(feature = "media-processing-image")]
mod semaphore;

#[cfg(feature = "media-processing-image")]
pub use image::ImageProcessProvider;
