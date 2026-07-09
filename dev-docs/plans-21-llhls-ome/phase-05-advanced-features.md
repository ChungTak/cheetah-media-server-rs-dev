# Phase 05 — 高级特性

- **目标**: 实现 DRM/CENC 加密、DVR 时移回放、SCTE-35 Marker/CUE 广告事件、WebVTT 字幕 Track
- **OME 参考**: `llhls_chunklist.cpp::MakeExtXKey` / `fmp4_storage.cpp DVR` / `marker_box.h` / `webvtt_packager`
- **优先级**: 按需实现，本阶段各子特性可独立交付

---

## 1. DRM/CENC 支持

### 1.1 OME 实现概要

OME 支持 CBAF (Common Encryption) 模式：

```cpp
// EXT-X-KEY 标签生成
#EXT-X-KEY:METHOD=SAMPLE-AES,
  URI="data:text/plain;base64,<PSSH_BOX>",
  KEYID=0x<KEY_ID>,
  KEYFORMAT="urn:uuid:<SYSTEM_ID>",
  KEYFORMATVERSIONS="1"

// FairPlay
#EXT-X-KEY:METHOD=SAMPLE-AES,
  URI="skd://<key_uri>",
  KEYFORMAT="com.apple.streamingkeydelivery",
  KEYFORMATVERSIONS="1"
```

支持的 DRM 系统：
- **Widevine**: CENC/CBCS + PSSH box
- **FairPlay**: CBCS + SKD URI
- **PlayReady**: 未完全实现

### 1.2 实现方案

**core 层**:
- 新增 `drm.rs` 模块：
  ```rust
  pub enum DrmScheme { None, Cenc, Cbcs }
  pub enum DrmSystem { Widevine, FairPlay }
  pub struct DrmConfig {
      pub scheme: DrmScheme,
      pub key_id: [u8; 16],
      pub key: [u8; 16],
      pub iv: Option<[u8; 16]>,
      pub systems: Vec<DrmSystem>,
      pub fairplay_key_uri: Option<String>,
  }
  ```
- playlist 生成时根据 DrmConfig 追加 `#EXT-X-KEY` 标签

**module 层**:
- fMP4 muxer 扩展：支持 CENC sample encryption（在 moof/trun 中添加 saiz/saio/senc box）
- init segment 中添加 sinf/schm/tenc box
- 配置：
  ```yaml
  hls:
    drm:
      enabled: false
      scheme: "cbcs"
      key_id: "..."
      key: "..."
      widevine_enabled: true
      fairplay_enabled: false
      fairplay_key_uri: ""
  ```

### 1.3 范围限定

- 本阶段只做 **播放端加密**（EXT-X-KEY 标签 + init segment 标记）
- 实际 sample-level encryption 实现复杂度高，可分子阶段交付：
  1. 先实现 EXT-X-KEY 标签输出（播放器知道如何获取密钥）
  2. 再实现 sinf/tenc init segment 标记
  3. 最后实现 sample-level CENC 加密

---

## 2. DVR / 时移回放

### 2.1 OME 实现概要

```cpp
struct Config {
    bool dvr_enabled = false;
    ov::String dvr_storage_path;
    uint64_t dvr_duration_sec = 0;
};

// Segment 写入磁盘
bool SaveMediaSegmentToFile(const std::shared_ptr<FMP4Segment> &segment);
// 从磁盘加载旧 segment
std::shared_ptr<FMP4Segment> LoadMediaSegmentFromFile(uint32_t segment_number) const;
```

DVR 模式下：
- segment 在内存 ring 中保留正常数量（用于 live playlist）
- 旧 segment 写入磁盘文件
- `_HLS_rewind=YES` 请求时从磁盘加载旧 segment 元信息生成 playlist
- 旧 segment 请求时从磁盘读取数据

### 2.2 实现方案

**module 层**:
- 本地已有 `HlsFileOutputConfig`（磁盘切片模式）
- 扩展为 DVR 模式：
  ```yaml
  hls:
    dvr:
      enabled: false
      storage_path: "/tmp/hls_dvr"
      max_duration_secs: 3600    # 最大时移时长（秒）
  ```
- segment 从 ring 中淘汰时：
  - 如果 DVR 启用 → 写入磁盘 + 记录元信息
  - 否则 → 丢弃
- segment 请求时：
  - 先查 ring（内存）
  - 如果不在 ring → 查磁盘
  - 磁盘也没有 → 404

**与 _HLS_rewind 的关系**:
- `_HLS_rewind` 需要 DVR 支持才有意义
- DVR 未启用时 `_HLS_rewind` 退化为仅输出 ring 内 segments

---

## 3. Marker / CUE 事件

### 3.1 OME 实现概要

OME 支持 SCTE-35 类的广告插入标记：

```cpp
// Marker 结构
class Marker {
    int64_t _timestamp;
    ov::String _tag;  // CUE-OUT / CUE-IN / CUE-OUT-CONT
    uint32_t _duration_msec;
};

// Playlist 输出
#EXT-X-DATERANGE:ID="...",START-DATE="...",PLANNED-DURATION=30.0,
  SCTE35-OUT=0x...
#EXT-X-CUE-OUT:30.0
#EXT-X-CUE-OUT-CONT:ElapsedTime=5.0,Duration=30.0
#EXT-X-CUE-IN
```

### 3.2 实现方案

**core 层**:
- 新增 `marker.rs` 模块：
  ```rust
  pub enum MarkerKind { CueOut { duration_secs: f64 }, CueIn, CueOutCont { elapsed: f64, duration: f64 } }
  pub struct Marker { pub timestamp_ms: i64, pub kind: MarkerKind }
  ```
- playlist 生成时在对应 segment/part 处插入 marker 标签

**module 层**:
- 通过 `EngineContext` 接收外部 CUE 事件（API 触发或 data track）
- 将 marker 关联到当前 segment

**触发方式**:
- Control API：`POST /api/streams/{key}/marker` 注入 CUE 事件
- Data Track：RTMP onCuePoint / RTSP data track
- 定时器：定时插入广告标记（scheduler，不在本阶段范围）

---

## 4. WebVTT 字幕 Track

### 4.1 OME 实现概要

OME 支持 WebVTT 字幕作为独立 media track：

```cpp
// VTT packager 绑定到 reference track（视频）的 segment 边界
// 每个视频 segment 对应一个 VTT segment
// Master playlist 中：
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="English",
  DEFAULT=YES,AUTOSELECT=YES,LANGUAGE="en",
  URI="chunklist_2_subtitle_key_llhls.m3u8"
```

### 4.2 实现方案

**前置**: Per-Track Chunklist（Phase 04）

**module 层**:
- 新增 `VttMuxer` 结构：
  ```rust
  struct VttMuxer {
      reference_track_id: u32,  // 绑定的视频 track
      segments: Vec<VttSegment>,
      cues: Vec<VttCue>,
  }
  ```
- 视频 segment 切割时同步切割 VTT segment
- VTT segment 内容格式：
  ```
  WEBVTT

  00:00:01.000 --> 00:00:04.000
  Hello world
  ```

**输入源**:
- RTMP data track（onTextData）
- RTSP subtitle track
- External API 注入

### 4.3 范围限定

- 本阶段仅支持 WebVTT（最通用的字幕格式）
- 不支持 CEA-608/708 closed captions（需要视频解码）
- 字幕 track 切片对齐到视频 segment 边界

---

## 5. 涉及文件变更

| 层 | 文件 | 变更 |
|----|------|------|
| core | 新增 `drm.rs` | DRM 配置模型 + EXT-X-KEY 标签生成 |
| core | 新增 `marker.rs` | Marker/CUE 事件模型 + 标签生成 |
| core | `playlist.rs` | DRM/Marker 标签输出 |
| module | `muxer.rs` | DVR segment 持久化 + Marker 关联 |
| module | `config.rs` | DRM / DVR / CUE 配置 |
| module | 新增 `vtt_muxer.rs` | WebVTT segment 打包 |
| module | `module.rs` | VTT track 路由 + CUE event 处理 |

---

## 6. 测试计划

| 测试 | 层 | 方法 |
|------|-----|------|
| EXT-X-KEY 标签格式 | core | 单元测试：Widevine/FairPlay 标签正确 |
| DVR segment 持久化 | module | 集成测试：segment 淘汰后磁盘文件存在 |
| DVR segment 读取 | module | 集成测试：请求旧 segment 从磁盘返回 |
| CUE-OUT / CUE-IN 标签 | core | 单元测试：Marker 正确输出到 playlist |
| VTT segment 对齐 | module | 单元测试：VTT 切片时间与视频一致 |

---

## 7. 子特性优先级

| 子特性 | 优先级 | 理由 |
|--------|--------|------|
| DVR / 时移 | 高 | 已有磁盘输出基础，_HLS_rewind 依赖 |
| CUE / Marker | 中 | 广告插入是商业化关键需求 |
| DRM / CENC | 中 | 版权保护需求，但实现复杂 |
| WebVTT 字幕 | 低 | 依赖 per-track 基础，使用场景较少 |

---

## 8. 完成标准

- [x] DRM: EXT-X-KEY 标签正确输出，播放器能触发 license 请求
- [x] DVR: 旧 segment 可从磁盘回放，_HLS_rewind 输出完整历史
- [x] CUE: API 注入 CUE 事件后 playlist 包含对应标签
- [x] VTT: 字幕 segment 与视频对齐，master playlist 包含 SUBTITLES 组
