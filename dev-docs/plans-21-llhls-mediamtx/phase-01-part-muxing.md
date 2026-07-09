# Phase 01 — Part 级别 fMP4 切片与 LLHLS Playlist 标签集成

- **状态**: 待实现
- **前置**: 现有 fMP4 muxer + SegmentRing + LowLatencyState 数据模型
- **目标**: 实现 LLHLS 的核心基础——将媒体流切割为 200-500ms 的 partial segments (parts)，并在 playlist 中生成完整的 LLHLS 标签
- **mediamtx 参考**: gohlslib `PartMinDuration` + `MuxerVariantLowLatency`

---

## 1. 需求分析

### 1.1 mediamtx/gohlslib 的 Part 切片模型

mediamtx 通过 gohlslib 实现 part 切片：
- `PartMinDuration: 200ms` — 每个 part 的最小时长
- 每个 part 是独立的 fMP4 fragment（`moof + mdat`）
- Part 在关键帧到达时标记 `independent=true`
- 完整 segment 由多个 parts 组成，关键帧对齐切割 segment 边界
- Segment 完成后，其 parts 仍保留在 playlist 中供客户端追赶

### 1.2 本地现状

- `Fmp4Muxer::write_segment()` 只能生成完整 segment 粒度的 fMP4
- `LowLatencyState` 有 `HlsPart` 模型和标签生成方法，但未被调用
- `StreamMuxer::push_frame()` 仅在关键帧+时长阈值时切割 segment

### 1.3 目标状态

- 新增 part 级别切片逻辑：每 `part_target_ms` 生成一个 fMP4 part
- Part 切片与 segment 切片解耦：part 按时长切割，segment 按关键帧切割
- Playlist 输出包含完整 LLHLS 标签集

---

## 2. 设计方案

### 2.1 Core 层：Part 切片状态机

**文件**: `crates/protocols/hls/core/src/ll_hls.rs`

扩展 `LowLatencyState`，增加 part 切片判定逻辑：

```rust
pub struct LowLatencyState {
    // 已有字段
    pub parts: Vec<HlsPart>,
    pub part_target_secs: f64,
    pub part_seq: u64,
    pub parent_segment_seq: u64,

    // 新增字段
    current_part_samples: Vec<Fmp4Sample>,   // 当前 part 累积的采样
    current_part_start_dts: Option<i64>,     // 当前 part 起始 DTS
    current_part_has_keyframe: bool,         // 当前 part 是否包含关键帧
    completed_parts: VecDeque<HlsPart>,      // 已完成 parts 的环形缓冲
    max_completed_parts: usize,             // 保留的历史 parts 上界
}
```

新增方法：

```rust
impl LowLatencyState {
    /// 喂入一个采样，返回是否应该切割 part
    pub fn should_cut_part(&self, sample_dts_ms: i64) -> bool;

    /// 完成当前 part，返回生成的 HlsPart
    pub fn finalize_part(&mut self, data: Bytes, independent: bool) -> HlsPart;

    /// 新 segment 开始时，将当前 parts 归档到 completed_parts
    pub fn on_segment_boundary(&mut self, new_segment_seq: u64);

    /// 获取指定序号的 part 数据
    pub fn get_part_data(&self, seq: u64) -> Option<&HlsPart>;
}
```

### 2.2 Core 层：fMP4 Part 封装

**文件**: `crates/protocols/hls/core/src/fmp4_mux.rs`

扩展 `Fmp4Muxer`，新增 part 级别输出：

```rust
impl Fmp4Muxer {
    /// 生成单个 part 的 fMP4 fragment (moof + mdat)，不含 styp
    pub fn write_part(&mut self, samples: &[Fmp4Sample]) -> Bytes;
}
```

Part 与 segment 的区别：
- Part: 仅 `moof + mdat`，无 `styp` 头
- Segment: `styp + moof + mdat`（完整 segment 由所有 parts 拼接而成）

### 2.3 Core 层：Playlist 集成 LLHLS 标签

**文件**: `crates/protocols/hls/core/src/playlist.rs`

扩展 `PlaylistBuilder`，新增 LLHLS playlist 生成：

```rust
impl PlaylistBuilder {
    /// 生成包含 LLHLS 标签的 media playlist
    pub fn build_media_ll(
        ring: &SegmentRing,
        ll_state: &LowLatencyState,
        container: HlsContainer,
        stream_prefix: &str,
    ) -> String;
}
```

生成的 playlist 包含：
- `#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=<3*part_target>`
- `#EXT-X-PART-INF:PART-TARGET=<part_target_secs>`
- 每个已完成 segment 后附带其 `#EXT-X-PART` 标签
- 当前未完成 segment 的 `#EXT-X-PART` 标签
- `#EXT-X-PRELOAD-HINT:TYPE=PART,URI="..."`（Phase 02 实现，此处预留接口）

### 2.4 Module 层：StreamMuxer Part 切片集成

**文件**: `crates/protocols/hls/module/src/muxer.rs`

扩展 `StreamMuxer`，在 `push_frame()` 中增加 part 切片逻辑：

```rust
pub struct StreamMuxer {
    // 已有字段...

    // 新增 LLHLS 字段
    ll_state: Option<LowLatencyState>,       // None = 传统 HLS 模式
    part_samples_buffer: Vec<Fmp4Sample>,    // 当前 part 的采样缓冲
}

impl StreamMuxer {
    pub fn push_frame(&mut self, frame: &AVFrame) -> Vec<MuxerOutput> {
        // 1. 编码帧 → Fmp4Sample
        // 2. 累积到 part_samples_buffer
        // 3. 检查 should_cut_part()
        //    - 是 → finalize_part() → 输出 MuxerOutput::PartReady
        // 4. 检查 should_cut_segment()（关键帧+时长）
        //    - 是 → on_segment_boundary() → 输出 MuxerOutput::SegmentReady
    }
}

pub enum MuxerOutput {
    SegmentReady(Segment),
    PartReady(HlsPart),
}
```

### 2.5 Config 层：LLHLS 配置项

**文件**: `crates/protocols/hls/module/src/config.rs`

```rust
pub struct HlsModuleConfig {
    // 已有字段...

    /// LLHLS 模式开关，默认 true
    pub ll_hls_enabled: bool,

    /// Part 目标时长（毫秒），默认 200
    pub part_target_ms: u64,

    /// Playlist 中保留的已完成 parts 数量上界，默认 50
    pub max_completed_parts: usize,

    /// Part hold-back 倍数（PART-HOLD-BACK = multiplier * part_target），默认 3.0
    pub part_hold_back_multiplier: f64,
}
```

对应 mediamtx 配置映射：
- `hlsVariant: lowLatency` → `ll_hls_enabled: true`
- `hlsPartDuration: 200ms` → `part_target_ms: 200`

---

## 3. 实现步骤

### Step 1: 扩展 fMP4 Muxer 支持 Part 输出

1. 在 `Fmp4Muxer` 中新增 `write_part()` 方法
2. Part 输出仅包含 `moof + mdat`，不含 `styp`
3. 复用现有的 `build_moof()` 和 `build_mdat()` 内部逻辑
4. 单元测试：验证 part 输出是合法的 fMP4 fragment

### Step 2: 扩展 LowLatencyState 切片判定

1. 新增 `should_cut_part()` — 基于 DTS 差值判定
2. 新增 `finalize_part()` — 生成 HlsPart 并重置状态
3. 新增 `on_segment_boundary()` — segment 边界处理
4. 新增 `completed_parts` 环形缓冲管理
5. 单元测试：验证 part 切片时机和序号递增

### Step 3: 集成 LLHLS Playlist 标签

1. 新增 `PlaylistBuilder::build_media_ll()`
2. 生成 `EXT-X-SERVER-CONTROL`、`EXT-X-PART-INF`、`EXT-X-PART` 标签
3. 确保 `EXT-X-PART` 的 `DURATION`、`URI`、`INDEPENDENT` 属性正确
4. 属性测试：验证生成的 playlist 格式符合 HLS 规范

### Step 4: StreamMuxer 集成 Part 切片

1. 在 `StreamMuxer` 中集成 `LowLatencyState`
2. `push_frame()` 增加 part 切片路径
3. 返回 `MuxerOutput` 枚举区分 segment/part 事件
4. 当 `ll_hls_enabled=false` 时走传统路径，无性能影响

### Step 5: 配置项与模式切换

1. `HlsModuleConfig` 新增 LLHLS 配置字段
2. YAML 配置解析支持
3. 环境变量覆盖支持
4. 配置变更触发 `ModuleRestartRequired`

---

## 4. 非标准兼容特性

### 4.1 Part 时长容忍度

标准要求 part 时长不超过 `PART-TARGET` 的 1.5 倍。实现中：
- 允许最后一个 part（segment 边界前）超过目标时长
- 音频 part 允许略微超长（AAC 帧 1024 samples ≈ 23ms 粒度）

### 4.2 关键帧对齐策略

- 如果关键帧到达时当前 part 时长不足 `part_target_ms * 0.5`，将关键帧合并到当前 part
- 避免生成过短的 parts（某些播放器对极短 part 处理不佳）

### 4.3 无视频流的 Part 切片

- 纯音频流也支持 part 切片
- 纯音频 part 的 `INDEPENDENT=YES` 始终为 true（音频帧均可独立解码）

### 4.4 编码格式降级

- 当容器为 TS 时，自动禁用 LLHLS（LLHLS 强制要求 fMP4）
- H265/VP9/AV1 自动选择 fMP4 容器并启用 LLHLS

---

## 5. 测试计划

| 测试类型 | 范围 | 验证点 |
|----------|------|--------|
| 单元测试 | `Fmp4Muxer::write_part()` | Part 输出是合法 fMP4 fragment |
| 单元测试 | `LowLatencyState` 切片判定 | 时长阈值、序号递增、边界处理 |
| 属性测试 | Playlist 格式 | LLHLS 标签语法正确、顺序正确 |
| 集成测试 | `StreamMuxer` 端到端 | 喂帧 → part + segment 输出 |
| 兼容测试 | hls.js LLHLS 模式 | 播放器能解析生成的 playlist |

---

## 6. 验收标准

1. `cargo test -p cheetah-hls-core` 全部通过
2. `cargo test -p cheetah-hls-module` 全部通过
3. 生成的 LLHLS playlist 包含正确的 `EXT-X-PART`、`EXT-X-SERVER-CONTROL`、`EXT-X-PART-INF` 标签
4. Part 时长在 `[part_target * 0.5, part_target * 1.5]` 范围内
5. 传统 HLS 模式（`ll_hls_enabled=false`）行为不变
6. fMP4 part 可被 hls.js 正确解析播放
