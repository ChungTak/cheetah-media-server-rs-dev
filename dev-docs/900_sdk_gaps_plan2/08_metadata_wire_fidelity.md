# 08 · R7：RTMP→HTTP-FLV Wire Metadata 保真与契约

> **Agent 用途**：阶段 6 主文档——关闭 STREAM-02 阻塞或给出 **官方不保真集合**。  
> **字段权威**：`cheetah-codec` `AVFrame` / `TrackInfo` / `flv_ingress.rs`。  
> **判断**：缺口在 wire 重建，不是模型缺字段。

---

## 1. 目标 / 非目标

**目标（二选一写死，禁止含糊）**：

| 策略 | 内容 |
| --- | --- |
| **A. 尽量保真** | 在 FLV 可表达范围内还原 duration/flags/side_data 等 |
| **B. 显式契约** | 维持技术限制，公布 `WIRE_NOT_PRESERVED` 集合 + conformance 断言“允许不同” |

**推荐**：A 能做多少做多少 + B 对剩余字段诚实。不得声称 full fidelity 却丢字段。

**非目标**：改 FLV 规范本身；跨所有协议 bit-exact payload（规范化 extradata 允许）。

---

## 2. 现状（connector-gaps §R7）

### 2.1 已保真（受测路径）

`track_id`, `media_kind`, `codec`, `format`, `pts`, `dts`, `timebase`, key-flag, `payload`  
track：`codec`, `clock_rate`, `sample_rate`, `channels`, `extradata`（可规范化）

### 2.2 丢失 / 改写

| 字段 | 行为 |
| --- | --- |
| `duration` / `duration_us` | → 0 |
| `origin` | → `FrameOrigin::Ingest` |
| `side_data` | 不整表保留；ingress 建 `SourceTimestamp::Rtmp` |
| 音频 `flags` | 恒 `START_OF_AU \| END_OF_AU` |
| 视频非关键 flags | DISCONTINUITY/CORRUPT/DROPPABLE/GENERATED 不保证 |
| `pts_us` / `dts_us` | 由 timebase 重算（应与 pts 一致，非独立通道） |
| extradata | H264 avcc / AAC ASC 规范化 |

证据路径：

```bash
rg -n 'duration|FrameOrigin|side_data|START_OF_AU|SourceTimestamp' \
  crates/foundation/cheetah-codec/src/flv_ingress.rs \
  crates/sdk/cheetah-connector/src/push/rtmp.rs | head -40
```

---

## 3. 官方契约表（proposed，实现时填最终）

### 3.1 必须保真（MUST）

| 字段 | 备注 |
| --- | --- |
| track_id / media_kind / codec / format | 禁止 Unknown 占位（已知码流） |
| pts / dts / timebase | 允许合法时间基转换但语义连续 |
| pts_us / dts_us | 必须 = `Timebase::to_micros` |
| keyframe flag | 必须 |
| payload 语义 | 允许 AnnexB/AVCC 规范化，format 同步 |
| TrackInfo codec / rate / channels / extradata 语义 | 允许规范化 |

### 3.2 尽力保真（SHOULD）

| 字段 | 方案 |
| --- | --- |
| duration / duration_us | 若 FLV 无字段：可用上一帧间距推断 **或** 列入 NOT_PRESERVED |
| 部分 flags | 在 tag 头可表达范围内映射 |

### 3.3 官方不保真（NOT_PRESERVED，须公开）

| 字段 | 原因 |
| --- | --- |
| `origin` 原始值 | ingress 语义固定 Ingest |
| 任意 `side_data` 全量 | FLV 无通用载体；仅允许协议特定 side_data |
| 非 FLV 可表达的自定义 flags | — |
| extradata 字节级 bit-exact | 规范化 |

```rust
// proposed 可测常量
pub const RTMP_HTTPFLV_WIRE_NOT_PRESERVED: &[&str] = &[
    "origin",
    "side_data.full_fidelity",
    "flags.non_key_video_extended",
    // duration 若未实现保真则列入
];
```

---

## 4. 实现选项

### 4.1 策略 A 增量（codec）

`flv_ingress` / egress 映射：

1. 评估 FLV script/tag 是否可带 duration；不能则文档 NOT_PRESERVED。  
2. flags：从 frame type / packet type 映射尽可能多。  
3. side_data：仅保留 `SourceTimestamp` 等协议相关；**不要**假装透传。  
4. origin：文档固定 Ingest；conformance 不比较 origin 相等。

### 4.2 策略 B 仅契约

1. 更新 `tests/metadata_conformance.rs`：MUST 字段全断言；NOT_PRESERVED 显式 `assert_ne` 或 skip 列表。  
2. rustdoc / plan 链接契约表。  
3. **禁止** 测试只覆盖 MUST 却宣传 full fidelity。

---

## 5. 测试清单

| ID | 用例 | 期望 |
| --- | --- | --- |
| T-MD-01 | L1 loopback 视频关键帧 MUST 字段 | 全过 |
| T-MD-02 | 音频 MUST 字段 | 全过 |
| T-MD-03 | pts_us 与 timebase 一致 | 全过 |
| T-MD-04 | NOT_PRESERVED 列表存在且测引用 | 契约锁 |
| T-MD-05 | duration 若 SHOULD 实现 | 非 0 或在 NOT_PRESERVED |
| T-MD-06 | extradata 规范化后可解码语义 | track ready |
| T-MD-07 | 禁止 codec=Unknown 绿测 | 失败回归 |

---

## 6. DoD（阶段 6）

- [ ] 契约表写入 rustdoc 或 `dev-docs` 并被测试引用  
- [ ] MUST 字段 L1 测全绿  
- [ ] NOT_PRESERVED 无 silent drop 却宣称保真  
- [ ] STREAM-02 可据此实现/关闭  
- [ ] 不破坏既有 metadata 测  

---

## 7. 衔接

- STREAM-01 可不阻塞于 R7；STREAM-02 阻塞。  
- R1/R2 新路径应复用同一契约框架（按协议分表）。  
