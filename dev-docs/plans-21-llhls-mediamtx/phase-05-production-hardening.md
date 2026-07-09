# Phase 05 — 生产加固：崩溃恢复、内嵌播放页、兼容性测试与性能优化

- **状态**: 待实现
- **前置**: Phase 04（高级特性）
- **目标**: 将 LLHLS 实现提升到生产级质量——自动故障恢复、开箱即用的调试工具、全面的播放器兼容性验证、性能基准与优化
- **mediamtx 参考**: muxer 崩溃重建 + 内嵌 hls.js 页面 + server_test.go 完整测试

---

## 1. 需求分析

### 1.1 Muxer 崩溃恢复

mediamtx 实现：
- `muxerInstance` 崩溃（panic/error）后 10 秒自动重建
- 重建期间所有关联 session 被关闭
- 新实例从当前流状态重新开始（不恢复历史 segment）
- 崩溃计数和日志记录

### 1.2 内嵌 hls.js 播放页

mediamtx 实现：
- 访问 `/{path}/` 返回内嵌 hls.js 的 HTML 页面
- 自动加载对应流的 playlist
- 开发调试和快速验证用途
- 内嵌 hls.js 库文件（离线可用）

### 1.3 播放器兼容性

LLHLS 需要验证的播放器矩阵：
- hls.js（Web，LLHLS 主要客户端）
- Safari（iOS/macOS 原生 HLS）
- VLC（桌面播放器）
- ffplay（命令行验证）
- ExoPlayer（Android）

### 1.4 性能要求

LLHLS 对性能敏感：
- Part 生成延迟 < 10ms（不能成为延迟瓶颈）
- Playlist 生成延迟 < 1ms
- Blocking request 唤醒延迟 < 5ms
- 支持 1000+ 并发 LLHLS 客户端

---

## 2. 设计方案

### 2.1 Muxer 崩溃恢复

**Module 层**:

```rust
pub struct MuxerHealth {
    /// 连续崩溃次数
    crash_count: u32,
    /// 最后崩溃时间
    last_crash_time: Option<Instant>,
    /// 重建间隔（指数退避）
    rebuild_delay: Duration,
    /// 最大重建间隔
    max_rebuild_delay: Duration,
}

impl MuxerHealth {
    /// 记录崩溃，返回下次重建延迟
    pub fn on_crash(&mut self) -> Duration;

    /// 重建成功，重置退避
    pub fn on_rebuild_success(&mut self);

    /// 是否应该放弃重建（连续崩溃过多）
    pub fn should_give_up(&self) -> bool;
}
```

崩溃恢复策略：
- 首次崩溃：立即重建
- 连续崩溃：指数退避（1s → 2s → 4s → 8s → 16s → 30s max）
- 连续 10 次崩溃：放弃重建，记录错误日志，等待人工干预
- 重建成功后重置退避计数

**Session 处理**:
- Muxer 崩溃时，所有关联 session 收到 503 响应
- Blocking 请求立即返回 503
- 客户端重试时如果 muxer 已重建，正常服务

### 2.2 内嵌 hls.js 播放页

**Driver 层**:

```rust
/// 内嵌播放页路由
/// GET /{ns}/{stream}/ → 返回 HTML 播放页
/// GET /_hls_player/hls.min.js → 返回 hls.js 库
```

HTML 模板：

```html
<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>LLHLS Player - {stream}</title>
    <style>
        body { margin: 0; background: #000; display: flex; align-items: center; justify-content: center; height: 100vh; }
        video { max-width: 100%; max-height: 100%; }
        #info { position: fixed; top: 10px; left: 10px; color: #fff; font-family: monospace; font-size: 12px; }
    </style>
</head>
<body>
    <video id="video" controls autoplay muted></video>
    <div id="info"></div>
    <script src="/_hls_player/hls.min.js"></script>
    <script>
        var video = document.getElementById('video');
        var info = document.getElementById('info');
        if (Hls.isSupported()) {
            var hls = new Hls({
                lowLatencyMode: true,
                liveSyncDurationCount: 3,
                liveMaxLatencyDurationCount: 6,
            });
            hls.loadSource('{playlist_url}');
            hls.attachMedia(video);
            hls.on(Hls.Events.MANIFEST_PARSED, function() { video.play(); });
            // 延迟统计
            setInterval(function() {
                if (hls.latency !== undefined) {
                    info.textContent = 'Latency: ' + hls.latency.toFixed(2) + 's | Buffer: ' + video.buffered.length;
                }
            }, 500);
        } else if (video.canPlayType('application/vnd.apple.mpegurl')) {
            video.src = '{playlist_url}';
        }
    </script>
</body>
</html>
```

**配置项**:

```rust
pub struct HlsModuleConfig {
    // 已有字段...

    /// 启用内嵌播放页，默认 true
    pub embed_player: bool,

    /// hls.js 版本（内嵌或 CDN URL）
    pub hls_js_url: Option<String>,
}
```

### 2.3 播放器兼容性矩阵

| 播放器 | LLHLS 支持 | 关键兼容点 |
|--------|-----------|-----------|
| hls.js ≥ 1.0 | ✅ 完整 | `lowLatencyMode: true`，blocking + delta + preload |
| Safari 17+ | ✅ 原生 | 要求 HTTPS，cookie 必须，HTTP/2 推荐 |
| Safari < 17 | ⚠️ 部分 | 不支持 delta updates，降级为传统 HLS |
| VLC 3.x | ❌ 不支持 | 忽略 LLHLS 标签，按传统 HLS 播放 |
| ffplay | ❌ 不支持 | 忽略 LLHLS 标签，按传统 HLS 播放 |
| ExoPlayer | ✅ 完整 | Android 原生支持 |

**兼容性保证**:
- LLHLS playlist 必须向后兼容传统 HLS 播放器
- 不支持 LLHLS 的播放器忽略 `EXT-X-PART` 等标签，正常播放完整 segment
- `EXT-X-VERSION` 设置为 9（LLHLS 最低要求）

### 2.4 性能优化

**Part 生成优化**:

```rust
/// 预分配 part buffer，避免频繁分配
struct PartBufferPool {
    buffers: Vec<BytesMut>,
    capacity: usize,  // 每个 buffer 的预分配大小
}
```

**Playlist 缓存**:

```rust
/// Playlist 缓存（避免每次请求重新生成）
struct PlaylistCache {
    /// 最新的完整 playlist
    full: Option<(u64, u64, Bytes)>,  // (msn, part_idx, content)
    /// 最新的 delta playlist
    delta: Option<(u64, u64, u64, Bytes)>,  // (msn, part_idx, skip_from, content)
}
```

缓存失效策略：
- 新 part 就绪时失效
- 新 segment 就绪时失效
- Blocking 请求唤醒时使用缓存（如果 msn/part 匹配）

**Blocking 唤醒优化**:

```rust
/// 使用 tokio::sync::Notify 替代轮询
/// 每流一个 Notify，part 就绪时 notify_waiters()
struct PartNotifier {
    notify: Arc<Notify>,
    current_msn: AtomicU64,
    current_part: AtomicU64,
}
```

---

## 3. 实现步骤

### Step 1: Muxer 崩溃检测与恢复

1. `StreamMuxer` 操作包装 `catch_unwind`（防止 panic 传播）
2. `MuxerHealth` 状态跟踪
3. 崩溃后清理：释放资源、通知 session
4. 定时器驱动重建
5. 集成测试：模拟 muxer panic 后自动恢复

### Step 2: 内嵌播放页

1. HTML 模板编译时嵌入（`include_str!`）
2. hls.js 文件嵌入或配置 CDN URL
3. URL 路由：`/{ns}/{stream}/` → 播放页
4. 模板变量替换：`{playlist_url}` → 实际 playlist URL
5. 功能测试：页面加载 + 播放启动

### Step 3: 播放器兼容性测试脚本

1. `dev-scripts/check_llhls_smoke.sh` — 基础 LLHLS 验证
2. 使用 `curl` 验证 blocking playlist 行为
3. 使用 `ffprobe` 验证 fMP4 part 格式
4. 使用 hls.js headless 验证端到端播放

### Step 4: Playlist 缓存

1. `PlaylistCache` 实现
2. Part/segment 就绪时失效缓存
3. 请求时优先返回缓存
4. 基准测试：缓存 vs 无缓存的 playlist 生成延迟

### Step 5: 性能基准测试

1. Part 生成延迟基准（目标 < 10ms）
2. Playlist 生成延迟基准（目标 < 1ms）
3. Blocking 唤醒延迟基准（目标 < 5ms）
4. 并发客户端压力测试（目标 1000+ 并发）
5. 内存使用基准（每流 LLHLS 开销）

### Step 6: 端到端集成测试

1. RTMP 推流 → LLHLS 播放完整链路
2. hls.js LLHLS 模式延迟测量
3. 多码率切换测试
4. 长时间运行稳定性测试（24h）

---

## 4. 非标准兼容特性

### 4.1 Playlist 版本降级

- 当检测到客户端不支持 LLHLS（无 `_HLS_msn` 参数）时，返回传统 playlist
- 传统 playlist 不包含 `EXT-X-PART` 等标签
- 同一流同时服务 LLHLS 和传统 HLS 客户端

### 4.2 Part 合并响应

- 某些播放器请求多个连续 parts 时，支持合并响应
- `Range: bytes=0-` 请求整个 segment（所有 parts 拼接）
- 兼容不理解 part 概念的播放器

### 4.3 Segment 完整性保证

- 即使 LLHLS 模式下，完整 segment 仍然可用
- Segment 文件 = 所有 parts 拼接 + styp 头
- 传统播放器请求 segment 时返回完整文件

### 4.4 优雅降级

- 服务器负载过高时自动降级：
  - 停止生成 parts，仅生成完整 segment
  - Blocking 请求立即返回（不等待）
  - Preload hint 省略
- 负载恢复后自动恢复 LLHLS 模式

### 4.5 Muxer 重建期间的服务

- Muxer 重建期间，返回最后已知的 playlist（标记为 stale）
- 已缓存的 segment/part 仍然可服务
- 避免重建期间所有客户端同时断开

---

## 5. 测试计划

| 测试类型 | 范围 | 验证点 |
|----------|------|--------|
| 单元测试 | MuxerHealth | 退避策略、放弃条件 |
| 单元测试 | PlaylistCache | 缓存命中/失效 |
| 集成测试 | 崩溃恢复 | panic → 重建 → 恢复服务 |
| 集成测试 | 内嵌播放页 | HTML 加载 + hls.js 初始化 |
| 性能测试 | Part 生成 | 延迟 < 10ms (p99) |
| 性能测试 | Playlist 生成 | 延迟 < 1ms (p99) |
| 性能测试 | 并发客户端 | 1000 并发无降级 |
| 兼容测试 | hls.js | LLHLS 模式端到端 |
| 兼容测试 | Safari | iOS/macOS 原生播放 |
| 兼容测试 | VLC/ffplay | 传统 HLS 降级播放 |
| 稳定性测试 | 长时间运行 | 24h 无内存泄漏、无崩溃 |

---

## 6. 验收标准

1. Muxer panic 后 ≤ 1s 自动重建，服务恢复
2. 连续崩溃时指数退避，不造成 CPU 风暴
3. 内嵌播放页可正常加载并播放 LLHLS 流
4. hls.js LLHLS 模式端到端延迟 < 1.5s（200ms part target）
5. VLC/ffplay 能以传统 HLS 模式正常播放同一流
6. 1000 并发 LLHLS 客户端时 CPU 使用率合理（< 50% 单核 per 流）
7. 24h 长时间运行无内存泄漏
8. `dev-scripts/check_llhls_smoke.sh` 通过

---

## 7. 性能目标

| 指标 | 目标值 | 测量方法 |
|------|--------|----------|
| Part 生成延迟 (p99) | < 10ms | 从最后一帧到 part 就绪 |
| Playlist 生成延迟 (p99) | < 1ms | 从请求到响应开始 |
| Blocking 唤醒延迟 (p99) | < 5ms | 从 part 就绪到响应发送 |
| 端到端延迟 (hls.js) | < 1.5s | 从编码到播放 |
| 内存开销 (per 流) | < 10MB | LLHLS 状态 + parts 缓冲 |
| 并发客户端 (per 流) | ≥ 1000 | 无降级正常服务 |
