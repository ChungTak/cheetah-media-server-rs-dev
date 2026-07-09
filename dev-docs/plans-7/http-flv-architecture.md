# HTTP-FLV 总体架构设计

- 状态：计划中
- 范围：固定 HTTP-FLV / WS-FLV 的 crate 边界、HTTP/WS 传输语义、FLV 封装复用方式、远端拉流方向和兼容测试边界。
- 完成标准：实现者能够据此拆出 `core + driver-tokio + module`，并在不复制 RTMP module 私有热路径逻辑的前提下完成播放输出和远端拉流。

## 架构目标

HTTP-FLV 本质是把 RTMP media tag payload 放进 FLV 文件/流格式，再通过 HTTP 长连接或 WebSocket binary message 传输。它不需要重新发明媒体帧模型，也不应该在 module 中复制一套 RTMP egress/ingest 逻辑。

首版固定为四层能力：

1. 共享媒体封装层：`cheetah-codec::flv` 提供 FLV header、完整 tag、`PreviousTagSize`、bounded demux 和 sequence header helper；`cheetah-rtmp-core` 提供 RTMP-compatible FLV payload adapter。
2. 协议 core：`cheetah-http-flv-core` 是 Sans-I/O 状态机，只处理请求解析结果、输出动作、FLV demux/mux 事件和错误状态。
3. Tokio driver：`cheetah-http-flv-driver-tokio` 独立监听 HTTP 端口，负责 HTTP/1.1、chunked、WebSocket upgrade、socket 读写、写队列和取消。
4. module：`cheetah-http-flv-module` 对接 engine，负责订阅本地 stream 输出 FLV，或从远端 HTTP/WS FLV 拉流并发布为本地 stream。

## Crate 与依赖方向

新增目录与 package：

```text
crates/protocols/http-flv/
  core/          # cheetah-http-flv-core
  driver-tokio/  # cheetah-http-flv-driver-tokio
  module/        # cheetah-http-flv-module
  testing/property-tests/  # cheetah-http-flv-property-tests
  fuzz/          # 独立 cargo-fuzz workspace
```

依赖方向固定为：

```text
cheetah-http-flv-module
  -> cheetah-http-flv-driver-tokio
  -> cheetah-http-flv-core
  -> cheetah-rtmp-core
  -> cheetah-codec

cheetah-http-flv-module -> cheetah-sdk -> cheetah-codec
```

禁止关系：

- `cheetah-http-flv-module` 不依赖 `cheetah-rtmp-module`。
- `cheetah-http-flv-core` 不依赖 Tokio、Axum、engine、SDK 或 socket。
- `cheetah-codec` 不依赖 HTTP、RTMP module、engine、SDK 或 runtime。

## HTTP / WebSocket 路由

对齐 SimpleMediaServer 的真实落地路径：

```text
GET /{app}/{stream}.flv
GET /{app}/{stream}.flv?type=enhanced
GET /{app}/{stream}.flv?type=fastPts
WebSocket /{app}/{stream}.flv
```

路由规则：

- 只接受 GET；OPTIONS 可返回 CORS 允许头。
- 路径必须以 `.flv` 结尾，去掉后缀后映射为 `StreamKey::new(app, stream)`。
- `type=enhanced` 启用 enhanced RTMP/FLV video signaling；`type=fastPts` 沿用 RTMP route 语义。
- HTTP 播放响应头：`200 OK`、`Content-Type: video/x-flv`、`Connection: keep-alive`、`Cache-Control: no-cache`、`Access-Control-Allow-Origin: *`。
- WebSocket 成功 upgrade 后只发送 binary message；每个 binary message 可以是一段 FLV bytes，不要求一个 message 等于一个 FLV tag。

## 播放输出语义

播放启动顺序固定为：

1. 等待或订阅本地 stream。
2. 发送 FLV header 与 `PreviousTagSize0`。
3. 如果启用 metadata，发送 `onMetaData` script tag。
4. 发送已知 video/audio sequence header。
5. 有视频且 codec 需要随机访问启动时，等待首个 keyframe 后转发媒体。
6. audio-only 流允许不等 keyframe 直接转发。
7. video-only 且开启 `enable_add_mute` 时，发送 AAC mute config，并按时间间隔补静音 AAC frame。

播放输出必须使用共享 adapter：

- `AVFrame + TrackInfo -> FlvTag`。
- timestamp 从 canonical timeline 导出为 RTMP/FLV ms。
- H264/H265/H266 参数集、NALU length size、AAC ASC、metadata、enhanced fourcc 都由共享 helper 处理。
- 对 unsupported codec，默认跳过对应 frame 并记录 warn；不得 panic。

## 远端拉流语义

pull job 输入支持：

```text
http://host/{app}/{stream}.flv
ws://host/{app}/{stream}.flv
```

首版不支持：

```text
https://...
wss://...
```

拉流流程：

1. module supervisor 根据配置启动 job。
2. driver client 连接远端 HTTP 或 WebSocket。
3. driver 将 HTTP chunked/body bytes 或 WebSocket binary message bytes 送入 core demux。
4. core/adapter 解析 FLV header、metadata、audio/video tag。
5. adapter 将 RTMP-compatible tag payload 复用 ingress helper 转为 `AVFrame + TrackInfo`。
6. module 使用 `PublisherApi::acquire_publisher` 获取独占发布租约，再写入 engine。
7. 连接关闭或错误后释放发布租约，并按配置退避重试；目标 stream 已被占用时停止该 job。

## 兼容策略

从 SimpleMediaServer 和真实客户端行为固定以下兼容点：

- FLV header 的 audio/video flags 只作为提示；真实 track 以 metadata、sequence header 和 media tag 为准。
- metadata 可以缺失；首个 tag 可以直接是 audio/video。
- `PreviousTagSize` 不匹配时记录兼容告警并继续；tag header 长度或 payload 越界时返回有界错误。
- demux remain buffer 默认上限 4 MiB，可配置但必须有最大值。
- HTTP response 支持 `Transfer-Encoding: chunked`，chunked parser 只在 driver 层出现。
- WebSocket 输入只接受 binary message；text message 忽略或关闭，由配置决定，默认关闭连接。
- 有视频时播放端默认等待 keyframe，避免以 delta frame 起播。
- enhanced video 识别 fourcc：`hvc1`、`vvc1`、`av01`、`vp08`、`vp09`。

## 具体任务

### A.1 固定协议三段式与 crate 边界

- [ ] 新增 `crates/protocols/http-flv/core`、`driver-tokio`、`module` 的设计说明与 Cargo package 命名。
- [ ] 明确 core 只处理 Sans-I/O 状态和 FLV protocol events，不持有 socket 或 engine state。
- [ ] 明确 driver 独立监听，不复用 `cheetah-control` 的 `ModuleHttpService`。
- [ ] 明确 module 不直接依赖 `cheetah-rtmp-module`。

### A.2 固定 HTTP/WS 路由、响应和播放启动语义

- [ ] 固定 `/{app}/{stream}.flv` 路由和 `type=enhanced|fastPts` query。
- [ ] 固定 HTTP 响应头和 WebSocket binary 传输方式。
- [ ] 固定 FLV header、metadata、sequence header、keyframe gate、mute AAC 的发送顺序。
- [ ] 固定 audio-only 与 unsupported codec 行为。

### A.3 固定兼容策略和测试分层

- [ ] 固定 FLV header flags、metadata 缺失、previous tag size mismatch 的兼容行为。
- [ ] 固定 chunked、分片、粘包、截断、重复、乱序、oversize 的 bounded robustness 断言。
- [ ] 固定标准样例做强行为断言，probe/fault 样例只做健康度和 bounded 断言。
- [ ] 固定 HTTPS/WSS 不进入首版。

## 完成后检查

```bash
cargo fmt
cargo check -p cheetah-http-flv-core
cargo check -p cheetah-http-flv-driver-tokio
cargo check -p cheetah-http-flv-module
```
