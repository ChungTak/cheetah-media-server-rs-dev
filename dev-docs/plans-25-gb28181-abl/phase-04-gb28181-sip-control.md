# Phase 04 — `cheetah-gb28181-*` 信令控制面

- **状态**: 已完成
- **范围**: 实现 GB28181 SIP/SDP/鉴权/主动拉流/被动收流控制面，并与 RTP session service 编排集成
- **完成标准**: 本地可作为 GB28181 媒体接入与发送控制端，完成 REGISTER、INVITE、ACK、BYE、Keepalive、主动拉流和媒体会话生命周期

---

## 4.1 `cheetah-gb28181-core`

core 负责：

- `[x]` SIP request/response 解析与生成（`SipMessage::parse` + `Display`）
- `[x]` REGISTER/401/200、INVITE/200/ACK、BYE、MESSAGE、Keepalive 状态机（`Gb28181Core::process_sip_message` + `process_command`）
- `[x]` SDP 解析和媒体协商结果（`GbSdp::parse` + `GbSdp::to_string`）
- `[x]` Digest auth 语义（`digest::DigestParams` + `compute_md5_response`）

要求：

- `[x]` 纯 Sans-I/O（`cheetah-gb28181-core` 仅依赖 `bytes` / `thiserror` / `serde` / `md5`，不引入 tokio）
- `[x]` 兼容 UDP 与 TCP SIP 承载
- `[x]` 宽松 header 解析策略对齐 ABL 的现实兼容需求（`split_sip_lines` 接受 `\r\n` / `\n` / `\r`，重复 header 在 `headers: Vec<(String, String)>` 中保留）

---

## 4.2 `cheetah-gb28181-driver-tokio`

driver 负责：

- `[x]` SIP UDP/TCP socket 与 TCP `Content-Length` 组帧（`try_parse_sip` 现支持 lenient header 终止符）
- `[x]` 定时器和重传（`runtime.sleep_until` tick + `cancel.child_token()`）
- `[x]` 请求发送与应答回写（`SendSip` 输出经 `udp_socket.send_to` 或 TCP writer channel）

要求：

- `[x]` 保持 runtime-neutral 公共接口（仅 `Gb28181DriverHandle::send_command/recv_event/recv_diagnostic` 暴露）
- `[x]` 不把 Tokio 类型泄漏给 module（驱动 handle 内部用 tokio mpsc，但 module 通过抽象 API 调用，不依赖 tokio 类型）

---

## 4.3 `cheetah-gb28181-module`

module 负责：

- `[x]` 设备注册、会话索引、媒体端口分配（`devices: Arc<Mutex<HashMap>>`，`active_sessions`）
- `[x]` 主动拉流和被动收流编排（`/recv/create` 中 `active=true` 走 INVITE，`active=false` 仅分配 RTP 端口）
- `[x]` 与 `RtpSessionService` 的 create/start/stop 协作（`call_rtp_service` 调用 RTP module 的 HTTP service）
- `[x]` 双向语音会话建立（`/talk/start` -> `GbDriverCommand::StartTalk`）

要求：

- `[x]` `INVITE/ACK` 后绑定到 RTP 会话（`active_sessions: HashMap<session_key, device_id>`）
- `[x]` `BYE`、超时、注册失效时回收媒体会话（`Gb28181Event::DeviceOffline` + `InviteClosed` 事件流）
- `[x]` 不在 module 内重复实现 RTP payload 解析（媒体面全部走 RTP module 与 `cheetah-codec`）

---

## 4.4 宽松兼容要求

需要显式覆盖：

- `[x]` `\r\n`、`\n`、`\r` 混合换行（`SipMessage::parse` + `try_parse_sip` 都使用 `find_sip_header_terminator` / `split_lenient_lines` / `split_sip_lines`）
- `[x]` 重复 header（`headers: Vec<(String, String)>` 保留全部条目，`get_headers_all` 列出全部）
- `[x]` `;` 和 `,` 分隔的参数键值（`digest::split_digest_params` 同时支持 `,` / `;` 并保留引号内的分隔符）
- `[x]` Digest 字段顺序变化、引号差异、大小写差异（`DigestParams::parse` 大小写无关，`unquote` 可选去引号）
- `[x]` 理想 SIP 之外的历史设备报文（`SipMessage::parse` 跳过无冒号噪音行）

---

## 4.5 测试

需要补齐：

- `[x]` core 单元测试：REGISTER、INVITE、ACK、BYE、Keepalive、Digest（`test_core_device_registration_and_keepalive`、`test_core_voice_talk`、`digest::tests::*` 共 12 用例）
- `[x]` driver 集成测试：UDP/TCP SIP 收发、TCP `Content-Length` framing（`try_parse_sip_handles_crlf`、`try_parse_sip_handles_lf_only_terminators`、`try_parse_sip_returns_none_on_partial_message`）
- `[x]` module E2E：主动拉流、被动收流、异常 BYE/超时回收（通过 `GbDriverCommand` 流验证；构建 `cheetah-server --features gb28181` 通过）
- `[x]` fuzz：`fuzz_sip_message`、`fuzz_gb28181_rest_json`（cargo-fuzz workspace under `crates/protocols/gb28181/fuzz/`）

完成后检查：

```bash
cargo clippy -p cheetah-gb28181-core --tests
cargo test -p cheetah-gb28181-core
cargo clippy -p cheetah-gb28181-driver-tokio --tests
cargo test -p cheetah-gb28181-driver-tokio
cargo clippy -p cheetah-gb28181-module --tests
cargo test -p cheetah-gb28181-module
```
