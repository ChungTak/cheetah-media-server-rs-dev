# 08 · Gap 6：Metadata-Preserving 端到端 Facade 契约

> **Agent 用途**：阶段 5 主文档——保证 `AVFrame`/`TrackInfo` 在 connector 路径上不被静默降级。  
> **字段权威**：`cheetah-codec` 的 `frame.rs` / `track.rs`。  
> **判断**：缺口在 facade + conformance，**不是** 模型缺字段。

---

## 1. 目标

在

```text
协议输入/输出
  → connector Push/Pull handle
  → PublisherSink / SubscriberSource
  → AVFrame / TrackInfo
```

路径上，关键 metadata **可断言保留**；禁止静默替换为 `Unknown` / `Timebase(1,1)` placeholder（除非协议侧确实未知，且错误/事件可观测）。

---

## 2. 契约 API（proposed）

两种落地方式，**任选其一**（推荐 A）：

### 方案 A：强化 `RuntimeConnector`（推荐）

```rust
#[async_trait]
pub trait RuntimeConnector: Send + Sync {
    async fn open_pull(…) -> Result<PullHandle, ConnectorError>;
    async fn open_push(…) -> Result<PushHandle, ConnectorError>;
}

impl PullHandle {
    /// Snapshot of tracks discovered so far (may grow after open).
    pub fn tracks(&self) -> Vec<TrackInfo>;
}

impl PushHandle {
    pub fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), ConnectorError>;
}
```

另提供 **conformance 测试模块**（非必须独立 trait）。

### 方案 B：显式 trait（与 gaps.md 一致）

```rust
#[async_trait]
pub trait MetadataPreservingConnector: RuntimeConnector {
    async fn open_pull_with_tracks(
        &self,
        protocol: Protocol,
        url: &str,
        options: ConnectorPullOptions,
    ) -> Result<(Vec<TrackInfo>, PullHandle), ConnectorError>;

    async fn open_push_with_tracks(
        &self,
        protocol: Protocol,
        url: &str,
        options: ConnectorPushOptions,
        tracks: Vec<TrackInfo>,
    ) -> Result<PushHandle, ConnectorError>;
}
```

若 open 时 tracks 尚未就绪：允许返回空 Vec，并提供 `wait_tracks(timeout) -> Vec<TrackInfo>`。

---

## 3. 必须保留的字段清单

### 3.1 `AVFrame`（逐字段）

| 字段 | 保留要求 |
| --- | --- |
| `track_id` | 稳定；音视频不串 |
| `media_kind` | 正确 Audio/Video |
| `codec` | 与码流一致；禁止无故 Unknown |
| `format` | 与入口封装视图一致或经 codec **显式** 规范化后文档化 |
| `pts` / `dts` | 不丢；单调性策略文档化 |
| `timebase` | 非占位；与 pts 单位一致 |
| `pts_us` / `dts_us` | 与 timebase 换算一致 |
| `duration` / `duration_us` | 未知可为 0；不得填随机值 |
| `flags` | keyframe/config/discontinuity 等关键位保留 |
| `payload` | 内容语义正确（允许规范化 Annex-B/AVCC 等，但需断言格式字段同步） |
| `side_data` | 若入口有，出口不得无故清空（允许过滤不支持项，需可测） |
| `origin` | 合理设置；不得无故改写为无关值 |

### 3.2 `TrackInfo`

| 类别 | 字段 |
| --- | --- |
| 身份 | track id、media kind、codec |
| 时基 | clock rate / timebase |
| 音频 | sample rate、channels |
| 视频 | width、height、fps（若可知） |
| 码率 | bitrate 若可知 |
| 配置 | `CodecExtradata`（H264/H265/AAC/…） |
| 状态 | readiness / config ready 标志（以源码字段名为准） |

实现前读取：

```bash
sed -n '250,340p' crates/foundation/cheetah-codec/src/track.rs
```

---

## 4. Conformance 测试设计

### 4.1 辅助断言

```rust
// tests/support/metadata_assert.rs
pub fn assert_frame_metadata_eq(expected: &AVFrame, actual: &AVFrame, opts: AssertOpts) {
    assert_eq!(expected.track_id, actual.track_id);
    assert_eq!(expected.media_kind, actual.media_kind);
    assert_eq!(expected.codec, actual.codec);
    assert_eq!(expected.format, actual.format);
    // timebase / pts：允许 opts.pts_tolerance_ticks
    assert_eq!(expected.flags.contains(KEY), actual.flags.contains(KEY));
    assert_eq!(expected.payload.as_ref(), actual.payload.as_ref()); // 或规范化后比较
}

pub fn assert_track_info_compatible(published: &TrackInfo, observed: &TrackInfo) { … }
```

### 4.2 场景

| ID | 场景 | 断言 |
| --- | --- | --- |
| T-M-01 | L1 loopback 视频关键帧 | codec/format/key/payload/extradata |
| T-M-02 | 音频帧 | sample rate/channels/codec |
| T-M-03 | push `update_tracks` 后 pull 可见 | TrackInfo 关键字段 |
| T-M-04 | config 帧 / extradata 后随媒体帧 | 不丢 SPS/PPS 语义 |
| T-M-05 | timebase 换算 | `pts_us` 与 `Timebase::to_micros` 一致 |
| T-M-06 | engine L0 smoke | 全字段相等（baseline） |
| T-M-07 | 禁止 placeholder 回归 | 对 fixture 断言 `codec != Unknown` |

### 4.3 Fixture

- 使用最小 H.264 IDR + SPS/PPS 与 AAC 配置帧（bytes 内嵌）。  
- 复用各协议 tests/testdata，避免新增大二进制。  
- 规范化策略（Annex-B vs AVCC）在测试中 **显式** 选择比较模式。

---

## 5. 协议适配器责任划分

| 层 | 责任 |
| --- | --- |
| 协议 module/driver | 从 wire 填充正确 codec/时间戳/extradata |
| `cheetah-codec` | 统一归一化、参数集缓存 API |
| connector | **不**改写字段；只编排；conformance 失败视为上游 bug 或 map bug |
| 外部 integrator | 不得被逼写 placeholder；若字段未知应是协议未协商 |

发现某协议路径用 placeholder 时：

1. 优先修协议 adapter / codec 映射。  
2. 不得在 connector 里 “猜” 一个假 SPS。  

---

## 6. 与 keyframe request 的关系

`PublisherSink::take_keyframe_requests` 在 loopback 中：

- 订阅侧触发 PLI/FIR（若协议支持）时计数增加。  
- conformance 可选断言：请求后下一个关键帧到达（best-effort）。  

非 P0 阻塞项，但 API 不得被 facade 吞掉。

---

## 7. DoD（阶段 5）

- [ ] Pull/Push handle 可查询 tracks（或 MetadataPreserving API）  
- [ ] 至少 L0 + 一条 L1 路径通过字段级断言  
- [ ] 无 “全 Unknown codec” 的绿测伪装  
- [ ] 文档写明允许的规范化（若 payload 字节不完全相等）  
- [ ] 与 Gap2/3 测试集成，不单测空壳  

---

## 8. 非目标

- 保证所有协议跨跳后 bit-exact payload（允许合法规范化）。  
- 修复历史 integrator 仓库里的 placeholder bridge（只提供上游契约）。  
- 引入第二套 frame 类型。  
