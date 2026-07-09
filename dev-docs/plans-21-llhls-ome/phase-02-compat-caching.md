# Phase 02 — 兼容模式与缓存优化

- **目标**: 实现 `_HLS_legacy` / `_HLS_rewind` 非标兼容模式，Gzip 响应压缩，Playlist 预生成缓存，细粒度 Cache-Control
- **OME 参考**: `llhls_chunklist.cpp::MakeChunklist(legacy/rewind)` / `UpdateCacheForDefaultChunklist` / `ResponseChunklist Cache-Control`

---

## 1. `_HLS_legacy` 模式

### 1.1 行为定义（OME 实现）

当请求 URL 包含 `_HLS_legacy=YES` 时，playlist 退化为传统 HLS：
- **不输出** `EXT-X-SERVER-CONTROL` 标签
- **不输出** `EXT-X-PART-INF` 标签
- **不输出** `EXT-X-PART` 标签（partial segment 信息）
- **不输出** `EXT-X-PRELOAD-HINT` 标签
- **不输出** `EXT-X-RENDITION-REPORT` 标签
- 仅输出已完成的 segment（未完成 segment 不列出）
- VERSION 降为 6

### 1.2 实现方案

**core 层**:
- `request.rs` 新增 `_HLS_legacy` 查询参数解析
- `HlsRequestKind::MediaPlaylist` 新增 `legacy: bool` 字段

**core 层 playlist.rs**:
- `PlaylistBuilder::build_media_ll` 新增 `legacy: bool` 参数
- 当 `legacy = true` 时跳过所有 LL-HLS 标签输出

**module 层**:
- 配置新增 `default_hls_legacy: bool`（默认 false）
- 请求中 `_HLS_legacy=YES` 覆盖默认值

---

## 2. `_HLS_rewind` 模式

### 2.1 行为定义（OME 实现）

当请求 URL 包含 `_HLS_rewind=YES` 时：
- playlist 输出 **所有** 保留的 segments（不限于 segment_count 窗口）
- 用于 DVR/时移观看场景
- 可与 `_HLS_legacy` 组合使用

### 2.2 实现方案

**core 层**:
- `request.rs` 新增 `_HLS_rewind` 查询参数解析
- `HlsRequestKind::MediaPlaylist` 新增 `rewind: bool` 字段

**module 层**:
- `StreamMuxer` 新增 `all_segments: Vec<SegmentMeta>` 保留所有历史 segment 元信息
- 当 `rewind = true` 时使用 all_segments 生成 playlist（而非仅 ring 内的）
- 配置新增 `default_hls_rewind: bool`（默认 false）
- 配置新增 `max_rewind_segments: usize`（默认 0 = 无限制，建议设上界如 1000）

---

## 3. Gzip 响应压缩

### 3.1 行为定义（OME 实现）

```cpp
auto encodings = request->GetHeader("Accept-Encoding");
if (encodings.IndexOf("gzip") >= 0 || encodings.IndexOf("*") >= 0) {
    gzip = true;
    content_encoding = "gzip";
}
```

仅对 **playlist/chunklist** 响应做 gzip，segment/part 数据不压缩（已经是二进制）。

### 3.2 实现方案

**driver 层**:
- `server.rs` 检测请求 `Accept-Encoding` 头是否包含 `gzip`
- 仅对 `content_type = "application/vnd.apple.mpegurl"` 的响应做 gzip
- 使用 `flate2` crate 的 `GzEncoder`（或 `miniz_oxide`）
- 添加 `Content-Encoding: gzip` 响应头

**module 层**:
- 可选：维护 gzip 版本的 cached playlist（减少 CPU 开销）
- 如果 playlist 缓存已实现，gzip 版本也缓存

### 3.3 依赖

- `flate2` crate（`default-features = false, features = ["miniz_oxide"]` 纯 Rust 实现）
- 或 `miniz_oxide` 直接使用

---

## 4. Playlist 预生成缓存

### 4.1 原理（OME 实现）

```cpp
void LLHlsChunklist::UpdateCacheForDefaultChunklist() {
    ov::String chunklist = MakeChunklist("", false, false, true);
    _cached_default_chunklist = chunklist;
    _cached_default_chunklist_gzip = ov::Zip::CompressGzip(chunklist.ToData(false));
}
```

每次 `AppendPartialSegmentInfo` 后更新缓存。非默认参数请求仍实时生成。

### 4.2 实现方案

**module 层 StreamMuxer**:
- 新增字段：
  ```rust
  cached_playlist: Option<String>,
  cached_playlist_gzip: Option<Bytes>,
  ```
- 每次 `finalize_part` 或 `finalize_segment` 后立即重建缓存
- `playlist()` 方法：
  - 无特殊参数（legacy/rewind/skip）→ 直接返回 cached_playlist
  - 有特殊参数 → 实时生成
- gzip 缓存仅在 driver 层请求 gzip 时使用

### 4.3 收益

- 多播放器同时请求同一流时，避免重复 playlist 构建
- 热路径 CPU 开销从 O(N_viewers) 降为 O(1)

---

## 5. 细粒度 Cache-Control

### 5.1 配置项（对标 OME）

```yaml
hls:
  cache_control:
    master_playlist_max_age: 0        # 0=no-cache, -1=不设置
    chunklist_max_age: 0              # 默认 no-cache
    chunklist_with_directives_max_age: 60  # 有 _HLS_msn 的请求
    segment_max_age: -1               # -1=不设置 Cache-Control
    partial_segment_max_age: -1       # -1=不设置
```

### 5.2 实现方案

**module 层 config.rs**:
- 新增 `CacheControlConfig` 结构体

**driver 层 server.rs**:
- 根据请求类型和配置设置不同的 `Cache-Control` 头
- Master playlist: `max_age` 或 `no-cache, no-store`
- Chunklist 无指令: `chunklist_max_age`
- Chunklist 有指令 (`_HLS_msn`): `chunklist_with_directives_max_age`
- Segment: `segment_max_age`
- Partial segment: `partial_segment_max_age`

### 5.3 CDN 模式交互

- 当 `cdn_secret` 配置后且请求通过 Bearer Token 验证：
  - Playlist 允许使用 `max-age`（CDN 边缘可缓存）
- 未配置 cdn_secret 时：
  - Playlist 始终 `no-cache`

---

## 6. 涉及文件变更

| 层 | 文件 | 变更 |
|----|------|------|
| core | `request.rs` | 新增 `_HLS_legacy` / `_HLS_rewind` 解析 |
| core | `playlist.rs` | `build_media_ll` 新增 legacy 参数 |
| module | `muxer.rs` | 新增 cached_playlist / cached_playlist_gzip + rebuild 逻辑 |
| module | `muxer.rs` | 新增 rewind segments 保留 |
| module | `config.rs` | 新增 CacheControlConfig / default_hls_legacy / default_hls_rewind |
| driver | `server.rs` | gzip 压缩 + Cache-Control 头 + Accept-Encoding 检测 |

---

## 7. 测试计划

| 测试 | 层 | 方法 |
|------|-----|------|
| legacy playlist 无 LL-HLS 标签 | core | 单元测试：build_media_ll(legacy=true) 验证无 PART/SERVER-CONTROL |
| rewind playlist 包含所有 segments | module | 单元测试：验证超出 ring 的历史 segment |
| gzip 压缩正确性 | driver | 集成测试：请求 Accept-Encoding: gzip 验证 Content-Encoding |
| playlist 缓存一致性 | module | 单元测试：push_frame 后 cached_playlist 更新 |
| Cache-Control 头正确 | driver | 集成测试：验证不同请求类型的 Cache-Control |

---

## 8. 完成标准

- [x] `_HLS_legacy=YES` 返回无 LL-HLS 标签的传统 playlist
- [x] `_HLS_rewind=YES` 返回所有历史 segments
- [x] Accept-Encoding: gzip 时响应被正确 gzip 压缩
- [x] playlist 缓存更新不丢失，多请求命中缓存
- [x] Cache-Control 头按配置正确设置
- [x] VLC（不支持 LLHLS）通过 legacy 模式正常播放
