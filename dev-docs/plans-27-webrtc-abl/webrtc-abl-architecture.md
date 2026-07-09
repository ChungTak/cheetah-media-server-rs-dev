# WebRTC ABL 对标架构设计

- **状态**: 已完成
- **范围**: 设计文档
- **参考**: `NetServerSendWebRTC.*`、`NetClientWebrtcPlayer.*`、`RtcpPacket.*`、`版本信息.txt`

## 设计目标

本阶段不是替换本地 WebRTC 技术栈，而是把 ABLMediaServer 中被发布记录反复修正过的工程兼容经验，沉淀为本项目的显式 compat 行为、配置项、测试样例和观测指标。

本地 WebRTC 继续保持以下边界：

- `cheetah-webrtc-core`: SDP/ICE/RTCP/RTP 相关 Sans-I/O 状态与纯逻辑。
- `cheetah-webrtc-driver-tokio`: UDP/TCP 收发、timer、framing、runtime task、backpressure。
- `cheetah-webrtc-module`: HTTP 路由、WHEP/WHIP API、引擎流绑定、鉴权、配置、会话注册表。
- `cheetah-codec`: 时间戳归一化、timebase 转换、Access Unit、参数集缓存/补发、输出封装视图。

## ABL 行为抽象

ABL 的 `NetServerSendWebRTC` 体现了五类能力：

1. **HTTP/WHEP 兼容**: `/rtc/v1/whep/?app=&stream=` 风格 URL、POST 创建会话、PATCH 交换 candidate、DELETE 结束会话、OPTIONS 快速返回并带 CORS。
2. **浏览器 SDP 兼容**: 从 offer 中提取 H264/H265/Opus payload，避免使用固定 payload；answer 中保留 ICE-lite、BUNDLE、rtcp-fb、transport-cc 等浏览器常用能力。
3. **媒体兼容**: H264/H265 RTP 打包；G711A/G711U 直通；AAC/MP3 转 Opus；Opus 使用 48kHz/stereo/960 sample 的浏览器友好参数。
4. **播放鲁棒性**: 播放对象不能因 HTTP 连接关闭立即销毁；真实播放结束或超时后再清理；回放音频/视频使用帧号派生 RTP timestamp。
5. **业务观测**: 暴露 WebRTC 播放 URL、播放对象、播放断开事件、播放时长、网络类型和客户端地址。

## 本地落点

| 能力 | 本地落点 | 说明 |
| --- | --- | --- |
| ABL 风格 WHEP URL | `crates/protocols/webrtc/module/src/http.rs`、`compat.rs` | 已有 ZLM/SMS URL 兼容基础，新增 ABL 别名解析和 OPTIONS 行为 |
| SDP payload 提取 | `crates/protocols/webrtc/core` 或 module 的 SDP 适配层 | 优先复用现有 SDP parser，缺失时补 core 纯函数 |
| codec 策略 | `crates/protocols/webrtc/module/src/codec_policy.rs` | 已有 Browser/Device/Passthrough profile，补 ABL 兼容策略 |
| 音频转码 | `cheetah-codec` 与 WebRTC module 输出适配 | 不在 module 热路径复制一套私有转码/时间戳逻辑 |
| 参数集补发 | `cheetah-codec` | SPS/PPS/VPS 缓存与 IDR 前补发统一放 codec |
| 会话生命周期 | `module` session registry + driver close events | HTTP 连接生命周期与 WebRTC 播放对象生命周期分离 |
| 端口范围 | `driver-tokio` config | 端口绑定在 driver，不进 core |
| 观测与事件 | `cheetah-sdk` 事件模型、module metrics/control API | 第一阶段复用现有事件总线和指标，不新增重量级 hook 系统 |

## 明确不迁移的内容

- 不迁移 ABL 的 libnice/OpenSSL/libsrtp2 组合，本地保持现有 WebRTC driver/core 技术路线。
- 不复制固定 fingerprint、固定 SSRC、固定 payload 这类历史实现。
- 不把 ABL 中 HTTP、ICE、RTP、转码、媒体源查找混在一个对象里的结构照搬到 module。
- 不在 module 中新增私有 NALU 参数集缓存或私有时间戳修正器。

## 配置模型建议

后续实现可在 `WebRtcModuleConfig` 中增加兼容配置，默认保持当前行为：

```rust
pub struct WebRtcAblCompatConfig {
    pub enabled: bool,
    pub public_webrtc_base_url: Option<String>,
    pub cors_allow_private_network: bool,
    pub whep_options_connection_close: bool,
    pub play_disconnect_min_duration_secs: u64,
    pub audio_transcode_policy: WebRtcAudioTranscodePolicy,
}

pub enum WebRtcAudioTranscodePolicy {
    BrowserPreferred,
    PassthroughWhenAccepted,
    ForceOpusForAacMp3,
}
```

这些类型属于计划草案；实现时应贴合现有配置文件风格和 serde 默认值约定。
