//! External signal integration contract tests.
//!
//! These tests verify the SDK media ports required by external signaling
//! projects (GB28181, ONVIF, Apple HomeKit, Matter). The `fake_*` tests use
//! in-memory doubles to validate parameter/contract paths. The `production_*`
//! tests start a real `Engine` with `RtpModule`, `RecordModule`, `ProxyModule`,
//! and a test fixture module to exercise real providers.
//!
//! 外部信令集成契约测试。
//!
//! `fake_*` 测试使用内存 double 验证参数/契约路径。
//! `production_*` 测试启动真实 Engine 与 RtpModule、RecordModule、ProxyModule
//! 以及测试 fixture module，以验证真实 provider。

mod common_production_contract;
mod fake_support;
mod production_support;

mod gb28181_fake_contract;
mod gb28181_production_contract;
mod homekit_fake_contract;
mod homekit_production_contract;
mod matter_fake_contract;
mod matter_production_contract;
mod onvif_fake_contract;
mod onvif_production_contract;
