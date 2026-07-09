# Phase 02 — `cheetah-rtp-core` 与 `cheetah-rtp-driver-tokio`

- **状态**: 已完成
- **范围**: 建立 RTP Sans-I/O core、Tokio driver、UDP/TCP/RTCP 收发与 ABL 风格的 TCP 兼容路径
- **完成标准**: 形成独立可测的 RTP server/client 运行骨架，支持 UDP/TCP、PS/TS/ES/JTT1078、RTCP 和 bounded 恢复

---

## 2.1 `cheetah-rtp-core`

core 负责：

- `[x]` `Input / Output / Event / Timer` 状态机（`RtpCoreInput` / `RtpCoreOutput` / `RtpCoreEvent` / `Tick`）
- `[x]` session、peer、SSRC、track router 管理（`RtpCore` + `ssrc_to_session` / `tcp_conn_to_session`）
- `[x]` RTP/RTCP 包级语义（`process_udp_packet` + `process_tcp_bytes` + `process_rtcp_packet`）
- `[x]` `send_only`、`recv_only`、`send_recv` 模式（`RtpTransportMode`），`voice_talk` / `udp_active` / `udp_passive` / `tcp_active` / `tcp_passive` 通过 `RtpConnectionType` 标注

必须避免：

- `[x]` Tokio、socket、`async fn`、系统时间 — `cheetah-rtp-core` 仅依赖 `cheetah-codec` + `bytes` + `thiserror` + `getrandom`
- `[x]` engine、HTTP、数据库、业务编排 — core 只输出 `RtpCoreOutput`，由 driver/module 处理 I/O 与编排

---

## 2.2 `cheetah-rtp-driver-tokio`

driver 负责：

- `[x]` UDP/TCP socket、listener、active connect、passive accept
- `[x]` TCP framing/deframing（自动识别 2-byte 长度头与 4-byte `$` interleaved）
- `[x]` RTCP 端口绑定和回包（`listen_rtcp_udp` 可选独立 socket，回写也优先使用该 socket）
- `[x]` bounded channel、backpressure、timeout、spawn（`mpsc(write_queue_capacity)` + `interval` tick + `CancellationToken`）

要求：

- `[x]` TCP 被动收流和被动推流都要支持（`tcp_listener.accept()` 路径和 `TcpStream::connect` 路径都已实现）
- `[x]` 单个会话可绑定媒体 socket 与 RTCP socket（`listen_rtcp_udp` + RTCP outputs 写到 RTCP socket）
- `[x]` 动态 `max_rtp_len` 学习留在 driver 的 I/O 上下文，不污染 core 公共接口（实现路径：core 内部记录 `max_rtp_len_observed`，driver 透传 `max_rtp_len_cap` 配置；公共 API 仅暴露诊断）

---

## 2.3 RTP / RTCP 细节

首版必须落地：

- `[x]` RTP seq、timestamp、ssrc 基本校验（`RtpPacket::parse` + 版本/空载/序列差检测）
- `[x]` RTCP SR、RR（5 秒 tick 自动生成 SR/RR）
- `[x]` report block 聚合（首版含 1 个 report block，highest_seq、jitter、loss 字段已置零，预留扩展位）
- `[x]` RTP port + 1 默认 RTCP 地址策略（默认 `listen_rtcp_udp = listen_udp.port + 1`，可独立配置）

后续预留但本阶段不强做：

- `[ ]` XR
- `[ ]` RTT/jitter/loss 指标增强（report block 已生成，统计计算留待后续阶段）

---

## 2.4 ABL 风格 transport compat

必须实现：

- `[x]` 2-byte 与 4-byte TCP RTP 头自动识别（`RtpTcpFraming::AutoDetect` + driver-tokio dual-mode read loop）
- `[x]` 坏流后的 bounded 重同步（`session.rs` 4 KiB scan window：known SSRC + PS pack-start）
- `[x]` 海康风格粘包/半包回归样例（`test_rtp_core_tcp_recovery_via_known_ssrc` + driver `try_parse_sip_returns_none_on_partial_message` 类比）
- `[x]` 单端口 ingress 的 PS/TS/ES/JTT1078 分派（`probe_rtp_payload` 自动落到 `SessionDemuxer::{Ts, Ps, Es}`）
- `[x]` source address 锁定与异常漂移诊断（`RtpCoreDiagnostic::SourceAddressChanged`）

---

## 2.5 测试

需要补齐：

- `[x]` core 单元测试：session 模式切换、SSRC 锁定、RTCP RR 生成（`test_rtp_core_*` 7 用例）
- `[x]` driver 集成测试：UDP、TCP active/passive、2-byte/4-byte framing、坏包恢复、RTCP 收发（`test_rtp_driver_udp_and_tcp_ingress`）
- `[x]` property tests：随机切分 TCP 流后 deframe 结果稳定（`prop_rtp_session::test_tcp_rtp_framing_arbitrary_splits`）
- `[x]` fuzz：`fuzz_rtp_header`、`fuzz_rtp_tcp_frame`（标准 cargo-fuzz workspace）

完成后检查：

```bash
cargo clippy -p cheetah-rtp-core --tests
cargo test -p cheetah-rtp-core
cargo clippy -p cheetah-rtp-driver-tokio --tests
cargo test -p cheetah-rtp-driver-tokio
```
