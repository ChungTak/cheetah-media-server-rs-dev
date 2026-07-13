//! External signal integration contract tests.
//!
//! These tests use independent test doubles to simulate the media calls made by
//! external signaling projects (GB28181, ONVIF, Apple HomeKit, Matter). They do
//! not implement any signaling protocol; they only verify that the SDK media
//! ports expose the flows required by those projects.
//!
//! 外部信令集成契约测试。
//!
//! 这些测试使用独立测试 double 模拟外部信令项目（GB28181、ONVIF、Apple HomeKit、Matter）
//! 的媒体调用。它们不实现任何信令协议，只验证 SDK 媒体端口是否能暴露这些项目所需的流程。

mod gb28181;
mod homekit;
mod matter;
mod onvif;
mod support;
