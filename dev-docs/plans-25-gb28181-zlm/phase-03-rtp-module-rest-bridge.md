# Phase 03 — RTP Module、REST API 与跨协议桥接

- **状态**: 已完成
- **范围**: 新增 `cheetah-rtp-module`，接入 engine，实现 RTP server/client、REST API、主动/被动模式、转推和鉴权前缓存
- **完成标准**: RTP 推流可发布为本地 stream，本地 stream 可转推到 RTP 远端，REST API 与配置可用，`cheetah-server` 可按 feature 启动模块

---

## 3.1 Module Factory 与配置

module manifest：

- `module_id`: `rtp`
- `display_name`: `RTP Module`
- `config_namespace`: `rtp`
- `routes_prefix`: `/api/v1/rtp`
- capabilities: `Publish`、`Subscribe`、`HttpApi`、`BackgroundJob`

配置重点：

- video/audio MTU
- G711 packet duration
- RTP max size
- RTCP timeout
- UDP recv buffer
- idle timeout
- pull/push jobs

---

## 3.2 ZLM 风格 REST API

路由：

```text
POST /api/v1/rtp/server/create
POST /api/v1/rtp/server/stop
POST /api/v1/rtp/client/create
POST /api/v1/rtp/client/start
POST /api/v1/rtp/client/stop
```

请求兼容字段：

- `port`
- `rtcpPort`
- `socketType`
- `payloadType`
- `transportMode`
- `conType`
- `ssrc`
- `appName`
- `streamName`
- `peerIp`
- `peerPort`
- `localPort`
- `onlyAudio`
- `senderInfos`
- `receiver`
- `recvStreamId`

---

## 3.3 RTP 推流进入本地引擎

要求：

- RTP ingress 后发布为本地 engine stream
- 无显式 stream 时默认 `/live/{ssrc}`
- 需要 publish auth 时，支持 bounded frame cache，避免鉴权返回前首帧丢失
- 多轨音视频全部进入 engine

---

## 3.4 本地流转 RTP 推流

要求：

- 支持 `es`、`ps`、`ts` 转推
- 支持 UDP/TCP active/passive
- 支持 `send_recv`
- 支持 `only_audio`
- 支持一个 SSRC 多目标发送
- 发送失败或 RTCP timeout 时自动清理 sender

---

## 3.5 App 与 Workspace 接入

改动点：

- 根 `Cargo.toml` workspace members 加入 RTP crates
- `apps/cheetah-server/Cargo.toml` 增加 feature `rtp`
- `apps/cheetah-server/src/main.rs` 注册 `RtpModuleFactory`
- `config.example.yaml` 增加 `rtp` 示例配置
- `SystemArchitecture.md` 增加 RTP 协议位置和依赖方向

---

## 3.6 Module 测试

测试场景：

1. 模块默认配置合法
2. `server/create` 开启 UDP/TCP active/passive 成功
3. `client/create` + `start` 主动发送成功
4. RTP-PS 接收后可在 RTSP 播放
5. RTP-TS 接收后可在 HLS 播放
6. 本地 RTMP/RTSP 输入流可转推到 RTP 远端
7. 同 SSRC 多目标发送互不拖累
8. publish auth 前缓存不丢首屏
9. `server/stop` 和 `client/stop` 正确释放资源

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-rtp-module --tests
cargo test -p cheetah-rtp-module
cargo clippy -p cheetah-server --features rtp
cargo test -p cheetah-server --features rtp
```
