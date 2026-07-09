# Phase 01: 共享 FLV/RTMP Adapter 抽取

- 状态：计划中
- 范围：扩展 `cheetah-codec::flv`，并把 RTMP module 中可复用的 FLV/RTMP payload 出站和入站能力抽到共享边界。
- 完成标准：RTMP module 与 HTTP-FLV module 能复用同一套 `AVFrame + TrackInfo <-> FLV/RTMP tag payload` helper；HTTP-FLV 不复制 RTMP module 私有 egress/ingest 逻辑。

## 目标文件与模块

重点修改：

```text
crates/foundation/cheetah-codec/src/flv.rs
crates/foundation/cheetah-codec/src/lib.rs
crates/protocols/rtmp/core/src/flv.rs
crates/protocols/rtmp/core/src/media.rs
crates/protocols/rtmp/core/src/lib.rs
crates/protocols/rtmp/module/src/egress.rs
crates/protocols/rtmp/module/src/ingest.rs
```

新增测试优先放在：

```text
crates/foundation/cheetah-codec/tests/flv_stream.rs
crates/protocols/rtmp/core/src/core/tests/flv_adapter.rs
```

## 共享类型

`cheetah-codec::flv` 需要提供完整流级模型：

```rust
pub struct FlvHeader {
    pub has_audio: bool,
    pub has_video: bool,
}

pub struct FlvTag {
    pub tag_type: FlvTagType,
    pub timestamp_ms: u32,
    pub payload: Bytes,
}

pub enum FlvDemuxEvent {
    Header(FlvHeader),
    Tag(FlvTag),
}

pub struct FlvDemuxer {
    max_buffer_bytes: usize,
}
```

行为规则：

- `FlvHeader::encode()` 固定输出 `FLV`、version `1`、flags、data offset `9` 和 `PreviousTagSize0 = 0`。
- `FlvTag::encode_with_previous_size()` 输出 11 字节 tag header、payload、4 字节 previous tag size。
- demux 支持任意分片输入，只有完整 header/tag 才产出事件。
- demux 对 payload 长度、remain buffer、record count 和 previous tag size mismatch 做有界处理。
- previous tag size mismatch 默认返回事件和 warning 标记，不直接拒绝整条流。

## RTMP-Compatible Adapter

`cheetah-rtmp-core` 需要公开不依赖 module 的 adapter：

```rust
pub enum RtmpFlvPayloadKind {
    Audio,
    Video,
    Data,
}

pub struct RtmpFlvPayload {
    pub kind: RtmpFlvPayloadKind,
    pub timestamp_ms: u32,
    pub payload: Bytes,
}
```

出站 helper：

- 从 `AVFrame + TrackInfo` 生成 `RtmpFlvPayload`。
- 从 `TrackInfo` 列表生成 metadata、video sequence header、audio sequence header、可选 mute AAC。
- 复用现有 RTMP module 对 enhanced video、H26x length-prefixed payload、Opus、MP3/G711/ADPCM 的编码策略。

入站 helper：

- 从 `FlvTag` / `RtmpFlvPayload` 解析 metadata 并更新 tracks。
- 从 audio/video tag payload 调用共享 ingress helper 生成 `AVFrame`。
- 沿用 RTMP ingress 的 timestamp normalizer、raw enhanced payload side data 和 track readiness 策略。

## 具体任务

### 1.1 扩展 `cheetah-codec::flv` 完整 tag 模型

- [ ] 增加 `FlvHeader`、`FlvTag`、`FlvDemuxer`、`FlvDemuxEvent` 和错误类型。
- [ ] 保留现有 `FlvTagBody` API 的兼容导出，或以类型别名/迁移方式减少下游改动。
- [ ] 增加 FLV header encode/decode、完整 tag encode/decode、extended timestamp、previous tag size 的单元测试。
- [ ] 增加 demux 任意分片、一次多 tag、metadata 缺失、oversize payload、truncated tag 的测试。

### 1.2 抽取 RTMP/FLV egress adapter

- [ ] 把 `build_metadata`、`build_video_config_payload`、H265/H266 config builder、enhanced video payload、mute AAC、playback codec support 判断移动到共享边界。
- [ ] 提供 `map_frame_to_rtmp_flv_payload(frame, mode, tracks)`，返回 audio/video FLV payload 与 timestamp。
- [ ] RTMP module 的 `egress.rs` 改为调用共享 adapter，再包装成 `RtmpCoreCommand`。
- [ ] 保持 RTMP module 现有测试语义不变，特别是 AV1 enhanced 不插 CTS、metadata 使用 fourcc、video-only mute AAC。

### 1.3 抽取 FLV ingest 到 AVFrame adapter

- [ ] 把 RTMP module ingest 中与 payload 解析、track 更新、timestamp normalize 相关的纯逻辑拆成可复用 helper。
- [ ] 提供 `apply_flv_metadata_to_tracks`、`handle_flv_video_ingest`、`handle_flv_audio_ingest` 风格 API。
- [ ] RTMP pull job 和 HTTP-FLV pull job 共享同一套 `PublishSession` 轨道和时间戳状态，或抽出协议无关 session state。
- [ ] 增加 FLV pull 输入回归：metadata + H264/AAC sequence headers + media tag 能生成 ready tracks 和 `AVFrame`。

## 测试要求

- `cheetah-codec` 测试只验证 FLV 容器和纯 helper，不引入 RTMP module、SDK、Tokio。
- `cheetah-rtmp-core` 测试验证 RTMP-compatible payload adapter，不访问 engine。
- `cheetah-rtmp-module` 回归必须证明迁移共享 adapter 后既有 RTMP publish/play/pull/push 行为不退化。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec flv
cargo clippy -p cheetah-rtmp-core
cargo test -p cheetah-rtmp-core flv
cargo clippy -p cheetah-rtmp-module
cargo test -p cheetah-rtmp-module
```
