//! Deterministic in-process WebRTC media loopback harness.
//!
//! This crate exposes the fixture used by `cheetah-connector` and the
//! `cheetah-webrtc-module` integration tests to exercise the WebRTC
//! packetizer/depacketizer path without ICE/DTLS/SRTP/UDP.
//!
//! 本 crate 提供 `cheetah-connector` 与 `cheetah-webrtc-module` 集成测试使用的
//! fixture，用于绕过 ICE/DTLS/SRTP/UDP 直接测试 WebRTC packetizer/depacketizer 路径。

pub mod media_loopback;

pub use media_loopback::MediaLoopbackHarness;
