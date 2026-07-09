# Phase 05: Transport Port Range Interop Fuzz

- **状态**: 未开始
- **目标**: 补齐 ABL 对部署端口范围和真实互操作问题的经验，并把异常输入纳入回归、property test 与 fuzz。

## 实现范围

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| WebRTC UDP 端口范围 | 未开始 | ABL 2025-08-08 增加 `WebRTCMinPort/MaxPort` |
| HTTP/HTTPS WebRTC 入口推导 | 未开始 | ABL 用端口奇偶区分 http/https，不建议照搬 |
| SDP 兼容样例 | 部分具备 | 继续补 ABL/browser/device offer fixtures |
| ICE/PATCH 异常输入 | 部分具备 | 结合 fuzz 与 module 互操作测试 |
| 性能与上界 | 部分具备 | 端口池、队列、重传窗口必须有上界 |

## 参考 ABL 行为

ABL 增加过 WebRTC UDP 发送端口范围配置，以适配防火墙/NAT 部署。ABL 还通过配置中的 `webrtcPort` 暴露访问 URL，并用端口奇偶推断 `http-webrtc`/`https-webrtc`。本项目不应复制奇偶规则，而应使用显式 scheme/base URL 配置。

## 开发任务

### Task 01: driver 端口范围配置

- **状态**: 未开始
- **建议文件**:
  - 修改: `crates/protocols/webrtc/driver-tokio` 配置与 socket 绑定模块
  - 修改: `crates/protocols/webrtc/module/src/config.rs`

验收点：

- 支持配置 `udp_port_min` 与 `udp_port_max` 或等价字段。
- min/max 无效时启动失败并给出明确配置错误。
- driver 在范围内绑定端口，失败时尝试下一个端口。
- 端口资源释放后可复用。
- core 不感知端口范围。

### Task 02: 显式 URL scheme 与 base URL

- **状态**: 未开始
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/config.rs`
  - 修改: WebRTC URL 构造逻辑

验收点：

- 支持显式 `public_webrtc_base_url`。
- 未配置时从 request scheme/host 或模块监听配置推导。
- 不用端口奇偶判断 HTTP/HTTPS。
- WHEP URL 与控制面展示 URL 一致。

### Task 03: ABL fixtures 与 fuzz 回归

- **状态**: 未开始
- **建议文件**:
  - 新增/修改: `crates/protocols/webrtc/testing/property-tests`
  - 新增/修改: `crates/protocols/webrtc/fuzz`
  - 新增: WebRTC SDP/HTTP fixtures

验收点：

- 覆盖 ABL 风格 WHEP URL。
- 覆盖大小写混合 codec、payload 不连续、缺失 opus、缺失 video codec。
- 覆盖 PATCH candidate body 为空、重复 candidate、ICE restart。
- fuzz 不触发 panic、无限循环或无界分配。

## 测试计划

```powershell
cargo test -p cheetah-webrtc-driver-tokio port
cargo test -p cheetah-webrtc-property-tests
cargo test -p cheetah-webrtc-module abl
cargo clippy -p cheetah-webrtc-driver-tokio
cargo clippy -p cheetah-webrtc-module
```

新增测试名称建议：

- `udp_port_range_binds_inside_configured_range`
- `udp_port_range_rejects_invalid_bounds`
- `public_webrtc_base_url_controls_whep_location`
- `abl_offer_fixture_does_not_panic`
- `trickle_patch_duplicate_candidate_is_idempotent`
