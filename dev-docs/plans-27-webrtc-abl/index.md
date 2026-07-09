# WebRTC ABL 对标增强计划索引

> **状态口径**: 本目录是设计与开发计划文档，默认不表示功能已经完成。`已有/复用` 表示本地 WebRTC 已具备同类基础能力；`未开始` 表示后续实现任务；`部分具备` 表示已有基础但需要按 ABL 兼容行为补齐。

## 背景

本计划参考 `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer` 的 WebRTC 相关实现与 `vendor-ref/ABLMediaServer-src-2026-05-09/版本信息.txt` 中的重要发布记录，对比本地 `crates/protocols/webrtc/{core,driver-tokio,module}` 的现状，规划下一阶段 WebRTC 协议增强。

本项目继续遵守仓库约束：WebRTC 仍采用 `core + driver + module` 三段式；`core` 保持 Sans-I/O；runtime、socket、timer、收发包在 `driver`；引擎接入、HTTP API、鉴权、资源绑定在 `module`；媒体时间戳、参数集缓存、封装视图优先沉淀到 `cheetah-codec`。

## ABL 参考结论

ABL 的 WebRTC 播放实现集中在 `NetServerSendWebRTC.*`，主要提供 WHEP 风格播放、HTTP OPTIONS/POST/PATCH/DELETE 会话流、ICE-lite、DTLS/SRTP、动态 payload 解析、H264/H265 RTP 打包、G711 直通、AAC/MP3 转 Opus、播放对象生命周期清理、CORS 与私网访问兼容、播放断开事件与流列表 URL 暴露。

本地已经具备较完整的 WebRTC 模块框架、WHEP/WHIP 路由、URL 兼容层、codec policy、simulcast/BWE/RTCP 指标、DataChannel/P2P 脚手架和 fuzz/property-test 基础。ABL 对标的价值不在于迁移 libnice/OpenSSL/libsrtp2，而是补齐真实浏览器、设备和历史客户端需要的非标准兼容行为。

## 文档清单

| 文档 | 状态 | 说明 |
| --- | --- | --- |
| [webrtc-abl-architecture.md](webrtc-abl-architecture.md) | 已完成 | ABL 行为抽象、本地分层落点、兼容边界 |
| [webrtc-abl-gap-analysis.md](webrtc-abl-gap-analysis.md) | 已完成 | ABL 与本地实现差距、优先级、风险 |
| [phase-01-signaling-http-whep-abl-compat.md](phase-01-signaling-http-whep-abl-compat.md) | 未开始 | WHEP/HTTP/CORS/OPTIONS/Location 兼容 |
| [phase-02-codec-payload-audio-timestamp.md](phase-02-codec-payload-audio-timestamp.md) | 未开始 | 动态 payload、音频策略、时间戳 |
| [phase-03-gop-bootstrap-parameter-set-playback.md](phase-03-gop-bootstrap-parameter-set-playback.md) | 未开始 | 首屏 GOP、参数集补发、回放帧号 |
| [phase-04-session-lifecycle-events-observability.md](phase-04-session-lifecycle-events-observability.md) | 未开始 | 会话生命周期、播放断开事件、观测面 |
| [phase-05-transport-port-range-interop-fuzz.md](phase-05-transport-port-range-interop-fuzz.md) | 未开始 | UDP 端口范围、传输兼容、互操作与 fuzz |

## 总任务状态

| 阶段 | 任务 | 状态 |
| --- | --- | --- |
| Phase 01 | 增强 ABL 风格 WHEP HTTP 信令兼容 | 未开始 |
| Phase 02 | 补齐 payload 协商、音频转码策略与时间戳兼容 | 未开始 |
| Phase 03 | 补齐首屏播放、参数集补发与回放帧号映射 | 未开始 |
| Phase 04 | 补齐播放生命周期、断开事件和查询观测 | 未开始 |
| Phase 05 | 补齐端口范围、传输互操作、回归样例与 fuzz | 未开始 |

## 建议执行顺序

1. 先做 Phase 01，使 HTTP/WHEP 入口能够覆盖 ABL 兼容客户端。
2. 再做 Phase 02 与 Phase 03，使媒体输出在浏览器和设备端更稳定。
3. 接着做 Phase 04，使会话清理、断开事件、控制面可观测。
4. 最后做 Phase 05，把端口范围、异常 SDP、真实样例和 fuzz 纳入回归。

## 最低验证命令

文档任务不需要运行 Rust 编译。后续实现阶段每个受影响 crate 至少执行：

```powershell
cargo fmt
cargo clippy -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-module
```

如果改动进入 `cheetah-webrtc-core`、`cheetah-webrtc-driver-tokio` 或 `cheetah-codec`，继续运行对应 crate 的 `clippy` 与 `test`。
