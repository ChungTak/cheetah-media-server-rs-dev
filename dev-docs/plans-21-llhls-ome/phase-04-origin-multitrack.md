# Phase 04 — Origin Mode 与多 Track 支持

- **目标**: 实现 CDN Origin Mode（session 池复用）、Per-Track Chunklist ABR 路由、Stream Key 防盗链验证
- **OME 参考**: `llhls_stream.cpp::CreateOriginSessionPool/GetSessionFromPool` / `llhls_publisher.cpp (origin_mode)` / Per-Track chunklist 路由 / stream_key 验证

---

## 1. Origin Mode（CDN 源站优化）

### 1.1 原理（OME 实现）

OME 的 Origin Mode 解决 CDN 边缘节点大量回源连接的问题：

```cpp
// 预创建 session 池（数量 = worker_count）
for (size_t i = 0; i < max_pool_size; i++) {
    auto session = LLHlsSession::Create(i, true, "", ...);
    AddSession(session);
}

// 请求到来时随机选择 session
std::shared_ptr<LLHlsSession> GetSessionFromPool() {
    size_t index = Random::GenerateUInt32() % max_pool_size;
    return GetSession(index);
}
```

关键区别：
- **非 Origin Mode**: 每个连接一个 session，session 跟踪播放器状态
- **Origin Mode**: session 池复用，不跟踪单个播放器状态，不验证 session key

### 1.2 实现方案

**module 层 config.rs**:
```yaml
hls:
  origin_mode: false          # CDN 源站模式开关
  origin_worker_count: 4      # Origin 模式 session 池大小
```

**module 层 module.rs**:
- 新增 `OriginSessionPool` 结构：
  ```rust
  struct OriginSessionPool {
      sessions: Vec<Arc<Mutex<SessionState>>>,
  }
  ```
- Origin Mode 下：
  - 不创建 per-connection session
  - 不设置 `Set-Cookie`
  - 不验证 session UID
  - Blocking 请求分配到 pool 中的 session 处理
  - segment/part URL 不带 `?uid=`

**driver 层 server.rs**:
- Origin Mode 下不读取/设置 Cookie
- 不验证 `?session=` 参数
- stream_key 嵌入 URL 代替 session 验证

### 1.3 Cache 行为

Origin Mode + CDN 场景下：
- Master playlist: 可设置 `max-age` 允许 CDN 缓存
- Chunklist 无指令: 短 `max-age`（如 1-2s）
- Chunklist 有指令: 长 `max-age`（如 60s，因为响应不会变）
- Segment/Part: 长 `max-age`（不可变内容）

---

## 2. Per-Track Chunklist（多 Track ABR）

### 2.1 原理（OME 实现）

OME 为每个 media track 生成独立的 chunklist：

```
/app/stream/llhls.m3u8                           → master playlist
/app/stream/chunklist_0_video_key_llhls.m3u8     → video track chunklist
/app/stream/chunklist_1_audio_key_llhls.m3u8     → audio track chunklist
/app/stream/init_0_video_key_llhls.m4s           → video init segment
/app/stream/seg_0_5_video_key_llhls.m4s          → video segment 5
/app/stream/part_0_5_2_video_key_llhls.m4s       → video track 0, seg 5, part 2
```

Master playlist 中：
```
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="audio",NAME="default",URI="chunklist_1_audio_key_llhls.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=...,RESOLUTION=...,CODECS="...",AUDIO="audio"
chunklist_0_video_key_llhls.m3u8
```

### 2.2 实现方案

**注意**: 本项目当前为单 track 单 playlist 模式。Per-Track 模式作为可选特性实现。

**module 层**:
- 新增 `MultiTrackMuxer` 结构（可选，当检测到多 track 时启用）：
  ```rust
  struct MultiTrackMuxer {
      tracks: HashMap<u32, TrackMuxer>,  // track_id → per-track muxer
      master_playlist: String,
  }

  struct TrackMuxer {
      track_id: u32,
      media_kind: MediaKind,
      ring: SegmentRing,
      ll_state: LowLatencyState,
      fmp4_muxer: Fmp4Muxer,
  }
  ```
- Master playlist 生成包含 `#EXT-X-MEDIA` 和 `#EXT-X-STREAM-INF`

**core 层 request.rs**:
- 新增解析模式支持 per-track URL 格式
- `HlsRequestKind` 变体增加 `track_id: Option<u32>` 字段

**driver 层 server.rs**:
- URL 路由支持 per-track chunklist 路径
- 按 track_id 查找对应 muxer 的 playlist/segment/part

### 2.3 向后兼容

- 默认模式：保持当前单 playlist 行为（音视频合并）
- Multi-track 模式：通过配置开启或自动检测（多视频轨 ABR 场景）
- 单音频+单视频时 per-track 模式仍可工作（Safari 原生播放器需要分轨）

---

## 3. Stream Key 防盗链

### 3.1 原理（OME 实现）

```cpp
// 流创建时生成随机 stream_key
_stream_key = ov::Random::GenerateString(8);

// segment/part URL 中嵌入 stream_key
// seg_0_5_video_<stream_key>_llhls.m4s

// 请求验证
if (stream_key != llhls_stream->GetStreamKey()) {
    response->SetStatusCode(http::StatusCode::NotFound);
    return;
}
```

目的：防止直接猜测 segment URL 进行盗链，只有获取过 playlist 的客户端才知道 stream_key。

### 3.2 实现方案

**module 层 muxer.rs**:
- 新增 `stream_key: String` 字段，流创建时随机生成（8 字符随机串）
- Part/Segment URI 格式变为：`part_{seq}_{stream_key}.m4s` / `seg_{seq}_{stream_key}.m4s`
- 或通过 query string：`part_5.m4s?k=<stream_key>`

**core 层 request.rs**:
- 解析 URL 时提取 stream_key 部分

**module 层 module.rs**:
- 收到 Segment/Part 请求时校验 stream_key
- 校验失败返回 404（不暴露流存在）

### 3.3 与 cdn_secret 关系

- `cdn_secret`: 验证 CDN 边缘节点的身份（Bearer Token），用于 playlist 请求
- `stream_key`: 验证播放器已获取 playlist（嵌入 segment/part URL），防止 URL 猜测

两者互补：cdn_secret 保护入口，stream_key 保护内容。

### 3.4 Origin Mode 下的 Stream Key

- Origin Mode 使用确定性 stream_key（基于 stream name hash），而非随机
- 因为 CDN 多个边缘节点回源，需要获取一致的 URL

---

## 4. 涉及文件变更

| 层 | 文件 | 变更 |
|----|------|------|
| module | `config.rs` | 新增 origin_mode / origin_worker_count / multi_track_enabled |
| module | `module.rs` | Origin session pool / multi-track routing |
| module | `muxer.rs` | stream_key 生成 + URL 嵌入 |
| core | `request.rs` | 新增 per-track URL 解析 + stream_key 提取 |
| driver | `server.rs` | Origin mode 响应行为 + per-track 路由 |

---

## 5. 测试计划

| 测试 | 层 | 方法 |
|------|-----|------|
| Origin Mode 不设 Cookie | driver | 集成测试：验证响应无 Set-Cookie |
| stream_key 校验通过 | module | 单元测试：正确 key 返回数据 |
| stream_key 校验失败 | module | 单元测试：错误 key 返回 404 |
| per-track URL 解析 | core | 单元测试：解析 chunklist_0_video.m3u8 |
| per-track master playlist | module | 单元测试：验证 EXT-X-MEDIA + STREAM-INF |
| Origin mode 确定性 key | module | 单元测试：相同 stream name → 相同 key |

---

## 6. 完成标准

- [x] Origin Mode 下多 CDN 边缘回源无 session 冲突
- [x] Stream Key 校验阻止 URL 猜测盗链
- [x] Per-Track Chunklist 生成正确的 master playlist
- [x] 各 track 独立切片，互不影响
- [x] Safari 原生 HLS 播放器通过分轨模式正常播放
