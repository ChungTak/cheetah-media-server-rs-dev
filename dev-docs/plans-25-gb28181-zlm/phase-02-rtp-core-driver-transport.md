# Phase 02 — RTP Core + Driver 传输层

- **状态**: 已完成
- **范围**: 新增 `cheetah-rtp-core` 与 `cheetah-rtp-driver-tokio`，实现 UDP/TCP RTP/RTCP、session 路由、主动/被动模式、TCP 恢复和 Ehome 兼容
- **完成标准**: 不接入 engine 的情况下，driver 可建立 RTP server/client、解析 UDP/TCP 输入、发送 RTP/RTCP、维护 SSRC/session 生命周期并兼容真实坏流

---

## 2.1 `cheetah-rtp-core` Sans-I/O 状态机

职责：

- RTP/RTCP session 路由
- SSRC、source address、payload mode、transport mode 管理
- TCP framing 和恢复状态
- session timeout、diagnostic、event 输出

核心模式：

- `recv_only`
- `send_only`
- `send_recv`
- `only_audio`
- `only_video`

---

## 2.2 UDP/TCP active/passive

支持：

- `udp_active`
- `udp_passive`
- `tcp_active`
- `tcp_passive`

设计要求：

- passive 模式支持先开端口再等对端连接/打洞
- active 模式支持先 create 再 start
- `socketType=both` 时支持同时收 UDP/TCP
- 未显式指定 stream 时支持按 `ssrc` 默认建流

---

## 2.3 TCP framing 与恢复

**兼容点**:

- 默认 2-byte RTP over TCP
- 兼容 RTSP-style 4-byte interleaved fallback
- 兼容 Ehome 私有头

**恢复策略**:

- 包长异常时进入 bounded search 状态
- 优先按 SSRC 搜索恢复
- 再按 PS system header 搜索恢复
- 恢复失败时丢弃当前坏片段或关闭连接

---

## 2.4 RTCP

支持：

- SR
- RR
- XR DLRR
- RTT、loss、jitter 统计

驱动要求：

- UDP 发送端支持 RR timeout 检测
- passive 场景支持 RTCP hole punching 后锁定 peer
- 默认 RTCP 端口为 RTP + 1，但允许单独配置

---

## 2.5 Driver 集成测试

测试场景：

1. UDP active/passive 收发 RTP
2. TCP active/passive 收发 RTP
3. TCP 半包、粘包、异常长度恢复
4. Ehome 头输入可正常恢复为 RTP
5. source address 漂移默认拒绝
6. RR timeout 触发 sender 关闭
7. SR/RR/XR 可生成和解析
8. `only_audio` / `only_video` 模式生效
9. 默认 `/live/{ssrc}` 建流可用

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-rtp-core
cargo test -p cheetah-rtp-core
cargo clippy -p cheetah-rtp-driver-tokio
cargo test -p cheetah-rtp-driver-tokio
```
