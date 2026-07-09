# WebRTC ABL 对标增强 — 任务清单

本 spec 落地 `dev-docs/plans-27-webrtc-abl` 五个阶段的 WebRTC ABL 兼容增强，参考 ABL MediaServer 的 WHEP 播放实现，补齐信令兼容、payload 协商、首屏播放、会话生命周期和传输互操作能力。

参考目录：
- `crates/protocols/webrtc/module/src/http.rs`（WHEP/WHIP HTTP 路由入口）
- `crates/protocols/webrtc/module/src/compat.rs`（URL 兼容层）
- `crates/protocols/webrtc/module/src/config.rs`（模块配置）
- `crates/protocols/webrtc/module/src/codec_policy.rs`（codec 策略）
- `crates/protocols/webrtc/core`（Sans-I/O 协议核心，SDP 解析）
- `crates/protocols/webrtc/driver-tokio`（runtime 驱动层）
- `crates/foundation/cheetah-codec`（媒体基础层，时间戳、参数集）

## 任务清单

- [x] 1. Phase 01: WHEP HTTP 信令 ABL 兼容
  - [x] 1.1 梳理并补齐 URL 解析测试：确保 `/rtc/v1/whep/?app=live&stream=camera01`、`/rtc/v1/whep?app=live&stream=camera01`、`/whep?app=live&stream=camera01` 均正确映射到 WHEP play；缺少 `app` 或 `stream` 时返回明确错误。修改 `crates/protocols/webrtc/module/src/compat.rs` 和 `crates/protocols/webrtc/module/src/http.rs`，补充单元测试
  - [x] 1.2 实现 OPTIONS 兼容响应：`OPTIONS /rtc/v1/whep/?app=live&stream=s` 返回 200，包含 `Access-Control-Allow-Origin`、`Access-Control-Allow-Methods`、`Access-Control-Allow-Headers`、`Content-Length: 0`；配置开启时追加 ABL 兼容的私网访问头；OPTIONS 不创建 WebRTC 播放会话。修改 `http.rs`、`module.rs`、`config.rs`
  - [x] 1.3 规范 POST/PATCH/DELETE 生命周期：POST 成功返回 `201 Created` 和 `application/sdp`；`Location` 指向稳定资源 URL；HTTP 连接关闭不触发播放会话销毁；POST 中流不存在、payload 不匹配、answer 生成失败时释放半初始化资源。修改 `http.rs` 和会话注册表
  - [x] 1.4 Phase 01 验证：`cargo fmt && cargo clippy -p cheetah-webrtc-module && cargo test -p cheetah-webrtc-module`

- [x] 2. Phase 02: Codec Payload 与音频时间戳
  - [x] 2.1 payload 解析收敛为纯函数：从 offer 中提取 `H264/90000`、`H265/90000`、`opus/48000` 对应 payload；codec 名大小写不敏感；找不到 payload 时返回结构化错误；answer 使用 offer 中协商成功的 payload，不回退到固定常量。修改 `crates/protocols/webrtc/core` SDP 模块，补测试
  - [x] 2.2 音频输出策略与配置：G711A payload 8 / G711U payload 0 可直通；AAC/MP3 面向 Browser profile 时优先输出 Opus；转码不可用时错误信息明确；Opus 输出使用 48kHz/stereo/960 sample frame。修改 `codec_policy.rs`、`config.rs`，视情况修改 `cheetah-codec`
  - [x] 2.3 timestamp 策略区分直播与回放：直播沿用单调递增/源时钟归一化；回放优先使用源帧号或源 PTS 派生 RTP timestamp；G711 timestamp 步进由采样率和 ptime 计算；Opus timestamp 使用 48kHz clock。修改 `cheetah-codec` 时间戳模块和 WebRTC module 输出适配
  - [x] 2.4 Phase 02 验证：`cargo fmt && cargo clippy -p cheetah-webrtc-core && cargo test -p cheetah-webrtc-core && cargo clippy -p cheetah-webrtc-module && cargo test -p cheetah-webrtc-module && cargo test -p cheetah-codec`

- [x] 3. Phase 03: GOP Bootstrap 与参数集补发
  - [x] 3.1 建立参数集缓存能力核查清单：确认 `cheetah-codec` 中 H264 可从 Annex-B 或 AVCC 识别 SPS/PPS、H265 可识别 VPS/SPS/PPS、IDR 前可生成包含参数集的输出视图、缓存大小有上界。补齐缺失能力
  - [x] 3.2 WebRTC 输出使用 codec bootstrap 视图：WebRTC 不直接维护私有 SPS/PPS/VPS map；新订阅者启动时优先发送可解码的关键帧序列；无关键帧时遵守 wait timeout 并返回可观测诊断；H264 B-frame filter 与参数集补发互不覆盖
  - [x] 3.3 增加真实样例回归：覆盖 IDR 缺 SPS/PPS、H265 IDR 缺 VPS/SPS/PPS、参数集变化后的更新。新增 WebRTC 或 codec 测试 fixtures
  - [x] 3.4 Phase 03 验证：`cargo fmt && cargo clippy -p cheetah-codec && cargo test -p cheetah-codec && cargo clippy -p cheetah-webrtc-module && cargo test -p cheetah-webrtc-module`

- [x] 4. Phase 04: 会话生命周期与事件观测
  - [x] 4.1 会话生命周期状态机文档化并测试：POST 创建信令会话；ICE/DTLS/SRTP 建立后进入播放会话；HTTP request drop 不等于 session close；DELETE/driver close/timeout/stream closed 均进入统一清理路径；半初始化失败不留 session registry 残留
  - [x] 4.2 播放断开事件与最小时长阈值：默认阈值可配置（参考 ABL 8 秒）；事件包含 stream key、session id、network type、remote addr、duration、close reason；短连接只记录指标不触发业务断开事件；复用现有 SDK 事件总线
  - [x] 4.3 控制面暴露 WebRTC URL 和会话摘要：流列表展示 WebRTC WHEP URL；URL 使用 `public_webrtc_base_url` 或 request host 推导；会话摘要展示协议、app、stream、remote addr、创建时间、播放时长、candidate 类型；不泄漏 DTLS fingerprint 私钥或认证 token
  - [x] 4.4 Phase 04 验证：`cargo fmt && cargo clippy -p cheetah-webrtc-module && cargo test -p cheetah-webrtc-module`

- [x] 5. Phase 05: 传输端口范围与互操作 Fuzz
  - [x] 5.1 driver 端口范围配置：支持 `udp_port_min` / `udp_port_max`；min/max 无效时启动失败并给出明确配置错误；driver 在范围内绑定端口，失败时尝试下一个；端口资源释放后可复用；core 不感知端口范围。修改 `crates/protocols/webrtc/driver-tokio` 配置与 socket 绑定模块
  - [x] 5.2 显式 URL scheme 与 base URL：支持显式 `public_webrtc_base_url`；未配置时从 request scheme/host 推导；不用端口奇偶判断 HTTP/HTTPS；WHEP URL 与控制面展示 URL 一致。修改 `config.rs` 和 URL 构造逻辑
  - [x] 5.3 ABL fixtures 与 fuzz 回归：覆盖 ABL 风格 WHEP URL、大小写混合 codec、payload 不连续、缺失 opus/video codec、PATCH candidate body 为空/重复/ICE restart；fuzz 不触发 panic/无限循环/无界分配。新增/修改 `crates/protocols/webrtc/testing/property-tests` 和 `crates/protocols/webrtc/fuzz`
  - [x] 5.4 Phase 05 验证：`cargo fmt && cargo clippy -p cheetah-webrtc-driver-tokio && cargo test -p cheetah-webrtc-driver-tokio && cargo clippy -p cheetah-webrtc-module && cargo test -p cheetah-webrtc-module`

## 验收标准

- ABL 风格 WHEP URL（`/rtc/v1/whep/?app=&stream=`）可正常播放
- OPTIONS 预检返回 CORS 且不创建会话
- POST 返回 201 + Location，HTTP 断开不销毁播放
- payload 从 offer 动态提取，不使用固定常量
- 音频策略可配置，G711 直通 / AAC→Opus 路径明确
- 直播/回放 timestamp 策略分离
- 新订阅者收到可解码关键帧序列（含参数集）
- 会话清理路径统一，半初始化不残留
- 播放断开事件可配置阈值
- UDP 端口范围可配置
- 所有受影响 crate 通过 `cargo clippy` 和 `cargo test`
