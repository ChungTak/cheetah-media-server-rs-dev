# Phase 03: RTSP Module 真实抓包集成回归

- 状态：计划中
- 范围：在 `crates/cheetah-rtsp-module/tests` 中用真实抓包 fixture 驱动 raw TCP/UDP publish 和 play，验证 module、driver、engine、RTSP TCP interleaved、UDP RTP/RTCP 的集成行为和非标准输入健康度。
- 完成标准：标准 fixture 能通过 module 进入 engine，并被 RTSP player 拉到 RTP；非标准/扰动 fixture 不会导致 rtsp module 崩溃或 engine health 失效。

## 目标文件

```text
crates/cheetah-rtsp-module/tests/rtsp_capture_replay.rs
crates/cheetah-rtsp-module/tests/common/mod.rs
crates/cheetah-rtsp-module/tests/support/rtsp_capture_fixture.rs
crates/cheetah-rtsp-module/tests/support/rtsp_test_harness.rs
```

如果继续沿用现有 `tests/common/mod.rs`，只能放通用、小型 helper。真实 fixture 解码、engine 启停、raw replay 策略应拆到 `tests/support/`，避免 `common/mod.rs` 继续膨胀。

## Module 测试边界

- module 测试可以使用 `tokio::net::TcpStream` 和 `tokio::net::UdpSocket`，但这些类型不得泄漏到 `cheetah-rtsp-module` 公共接口。
- raw replay 不解析原始 pcap，只读取 `.rtspcap` fixture。
- 测试 harness 必须动态保留 `127.0.0.1:0`，启动 `EngineBuilder` 并注册 `RtspModuleFactory`。
- 对含原始 URI authority 的 RTSP request，replay 前必须把 `rtsp://127.0.0.1:8554/...` 规范化为测试实际 listen 地址；同时保持 path/stream name 不变。
- 对 UDP replay，SETUP request 中的 `client_port` 必须改写为测试绑定的 UDP socket 端口；否则服务器会把 RTP/RTCP 发到 fixture 原始端口。

## 具体任务

### 3.1 Module TCP interleaved publish replay

- [x] 新增测试 harness：动态保留 listen 地址，启动 engine，注册 `RtspModuleFactory`。
- [x] 用 raw `TcpStream` 连接 RTSP listen 地址，按 standard TCP fixture 的 C2S control record replay `OPTIONS -> ANNOUNCE -> SETUP -> RECORD`。
- [x] 对 `ANNOUNCE` body 保留真实 SDP；只改写 request URI authority 和必要的 `Content-Length`。
- [x] 对 TCP interleaved publish record，按原始 record 边界写入；标准通过后新增 coalesced 写入模式。
- [x] 等待 engine stream snapshot 出现 active publisher 和非空 tracks。
- [x] 启动第二个 raw RTSP player，执行 `DESCRIBE -> SETUP RTP/AVP/TCP;interleaved=2-3 -> PLAY`，读取 `$` interleaved RTP frame。
- [x] 断言至少收到一个 RTP packet，`RtpPacket::parse` 成功，payload type 与 SDP track payload type 匹配。

### 3.2 Module UDP publish/play replay

- [x] 对 UDP standard fixture，publisher 连接先 replay control plane，并把 `SETUP Transport: client_port` 改写为测试 publisher RTP/RTCP socket。
- [x] 从 SETUP response 解析 `server_port`，把 fixture 的 `udp_publish_rtp` / `udp_publish_rtcp` datagram 发送到 module server port。
- [x] player 连接执行 `DESCRIBE -> SETUP RTP/AVP;client_port=x-y -> PLAY`，绑定 player RTP/RTCP socket。
- [x] 断言 player RTP socket 至少收到一个可解析 RTP packet。
- [x] 对 RTCP sender report / receiver report 只做最小断言：packet type 可读，收发不会导致连接关闭或 module stop。
- [x] 对 H264 UDP 标准样例，若连续收到多个同 SSRC RTP，断言 sequence 在未扰动 replay 下基本递增（当前样例集中无 H265 UDP 标准 fixture，H265 覆盖放在 TCP/fuzz 阶段收口）。

### 3.3 Probe/fault module 健康度回归

- [x] 对 AV1/VP8/VP9/H266/4K probe fixture，只要求 raw replay 后 rtsp module 仍为 `ModuleState::Running`。
- [x] 对截断、丢包、乱序、重复视图，只要求 engine health `is_live()` 和 `is_ready()` 仍为 true。
- [x] 如果连接被 module 主动关闭，测试接受关闭事件，并确认 module 可停止且 stop 后状态为 `Stopped`。
- [x] UDP 丢包视图覆盖：每 N 个 RTP datagram 丢弃一个、丢弃首个 FU fragment、丢弃 RTCP SR。
- [x] UDP/RTP 乱序视图覆盖：交换相邻 RTP datagram、对小窗口反序、重复旧 sequence number。
- [x] 不在 module 测试里复制媒体 payload 修复逻辑；遇到 codec 解析不足，只把 fixture 纳入 probe/fuzz，生产逻辑修复另开任务回到 `cheetah-codec` 或 RTSP media 边界。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-module --test rtsp_capture_replay
cargo test -p cheetah-rtsp-module --test publish_record
cargo test -p cheetah-rtsp-module --test udp_forwarding
cargo test -p cheetah-rtsp-module --test play_pause
```

如果新增测试在低性能环境偶发超时，优先收紧 fixture 前缀、减少 datagram 数量和明确事件等待条件，不放宽 module 生命周期断言。
