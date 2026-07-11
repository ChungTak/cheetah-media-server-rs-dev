//! Tokio driver for the SRT protocol.
//!
//! SRT 协议的 Tokio 驱动。

mod config;
mod driver;

pub use config::{SrtDriverConfig, SrtDriverEncryption};
pub use driver::{
    spawn_driver, SrtDriverCommand, SrtDriverEvent, SrtDriverHandle, SrtDriverStats, SrtPeerId,
};
