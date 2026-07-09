# Phase 02 — Blocking Playlist Reload、Delta Updates 与 Part 端点

- **状态**: 待实现
- **前置**: Phase 01（Part 级别切片与 LLHLS Playlist 标签）
- **目标**: 实现 LLHLS 的核心交付机制——客户端通过 `_HLS_msn`/`_HLS_part` 参数发起阻塞请求，服务端在新 part 就绪时立即响应，实现亚秒级延迟
- **mediamtx 参考**: gohlslib `Handle()` 内部的 blocking request 处理 + delta updates

---

## 1. 需求分析

### 1.1 Blocking Playlist Reload（LLHLS 核心机制）

客户端请求 playlist 时附带查询参数：
- `_HLS_msn=<N>` — 期望的最小 Media Sequence Number
- `_HLS_part=<M>` — 期望的最小 Part Index

服务端行为：
- 如果请求的 msn/part 已就绪 → 立即返回 playlist
- 如果尚未就绪 → 挂起请求（long-poll），直到对应 part 生成后返回
- 超时（如 `PART-HOLD-BACK * 3`）后返回当前最新 playlist

### 1.2 Delta Updates

客户端请求 playlist 时附带：
- `_HLS_skip=YES` — 请求增量 playlist
- `_HLS_skip=v2` — 请求增量 playlist（含 Rendition Report）

服务端行为：
- 返回 `EXT-X-SKIP:SKIPPED-SEGMENTS=<N>` 替代已知的旧 segments
- 减少 playlist 传输大小，降低延迟

### 1.3 Preload Hint

Playlist 末尾包含：
```
#EXT-X-PRELOAD-HINT:TYPE=PART,URI="partN.m4s"
```
告知客户端下一个 part 的 URI，客户端可提前发起请求。

### 1.4 mediamtx 实现模式

mediamtx 将所有 LLHLS 请求处理委托给 `gohlslib.Muxer.Handle(w, r)`：
- 内部解析 `_HLS_msn`、`_HLS_part`、`_HLS_skip` 查询参数
- 使用 Go channel 实现 blocking wait
- Playlist 和 segment/part 请求统一通过同一 Handle 入口

---

## 2. 设计方案

### 2.1 Core 层：请求路由扩展

**文件**: `crates/protocols/hls/core/src/request.rs`

扩展 `HlsRequestKind` 支持 LLHLS 查询参数：

```rust
pub enum HlsRequestKind {
    MasterPlaylist { namespace: String, stream_path: String },
    MediaPlaylist {
        namespace: String,
        stream_path: String,
        uid: Option<String>,
        // 新增 LLHLS 参数
        blocking: Option<BlockingParams>,
        skip: Option<SkipMode>,
    },
    Segment { namespace: String, stream_path: String, name: String },
    InitSegment { namespace: String, stream_path: String },
    // 新增：Part 请求
    Part { namespace: String, stream_path: String, part_name: String },
}

/// Blocking Playlist Reload 参数
pub struct BlockingParams {
    pub msn: u64,       // _HLS_msn
    pub part: Option<u64>,  // _HLS_part（可选）
}

/// Delta Update 模式
pub enum SkipMode {
    Yes,    // _HLS_skip=YES
    V2,     // _HLS_skip=v2
}
```

### 2.2 Core 层：Session 状态机扩展

**文件**: `crates/protocols/hls/core/src/session.rs`

扩展事件和命令：

```rust
pub enum HlsCoreEvent {
    // 已有...
    MasterPlaylistRequested { ... },
    MediaPlaylistRequested { ... },
    SegmentRequested { ... },
    InitSegmentRequested { ... },

    // 新增
    BlockingPlaylistRequested {
        stream_key: StreamKeyParts,
        uid: Option<String>,
        blocking: BlockingParams,
        skip: Option<SkipMode>,
        connection_id: u64,
    },
    PartRequested {
        stream_key: StreamKeyParts,
        part_name: String,
        connection_id: u64,
    },
}
```

### 2.3 Core 层：Blocking Wait 模型

**文件**: `crates/protocols/hls/core/src/ll_hls.rs`

新增 blocking wait 管理（Sans-I/O，不持有 channel）：

```rust
/// 等待中的阻塞请求
pub struct PendingBlockingRequest {
    pub connection_id: u64,
    pub target_msn: u64,
    pub target_part: Option<u64>,
    pub skip: Option<SkipMode>,
    pub deadline_ms: u64,  // 超时时间戳
}

impl LowLatencyState {
    /// 注册一个阻塞请求，返回是否可以立即满足
    pub fn register_blocking_request(&mut self, req: PendingBlockingRequest) -> BlockingResult;

    /// 新 part 就绪时，检查并返回可以唤醒的请求列表
    pub fn on_part_ready(&mut self, msn: u64, part_idx: u64) -> Vec<u64>; // connection_ids

    /// 检查超时的阻塞请求
    pub fn check_timeouts(&mut self, now_ms: u64) -> Vec<u64>; // connection_ids to timeout
}

pub enum BlockingResult {
    /// 数据已就绪，立即响应
    Ready,
    /// 数据未就绪，已注册等待
    Pending,
    /// 请求的 msn 已过期（太旧），返回当前最新
    Expired,
}
```

### 2.4 Core 层：Delta Updates Playlist 生成

**文件**: `crates/protocols/hls/core/src/playlist.rs`

```rust
impl PlaylistBuilder {
    /// 生成 delta update playlist（含 EXT-X-SKIP）
    pub fn build_media_ll_delta(
        ring: &SegmentRing,
        ll_state: &LowLatencyState,
        skip_mode: SkipMode,
        client_known_msn: u64,  // 客户端已知的最新 msn
        stream_prefix: &str,
    ) -> String;
}
```

Delta playlist 格式：
```
#EXTM3U
#EXT-X-TARGETDURATION:4
#EXT-X-VERSION:9
#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,CAN-SKIP-UNTIL=24.0,PART-HOLD-BACK=0.6
#EXT-X-PART-INF:PART-TARGET=0.2
#EXT-X-MEDIA-SEQUENCE:10
#EXT-X-SKIP:SKIPPED-SEGMENTS=5
#EXTINF:4.0,
seg10.m4s
#EXT-X-PART:DURATION=0.2,URI="part0.m4s",INDEPENDENT=YES
#EXT-X-PART:DURATION=0.2,URI="part1.m4s"
#EXT-X-PRELOAD-HINT:TYPE=PART,URI="part2.m4s"
```

### 2.5 Core 层：Preload Hint 生成

在 `LowLatencyState` 中新增：

```rust
impl LowLatencyState {
    /// 生成 EXT-X-PRELOAD-HINT 标签（指向下一个即将生成的 part）
    pub fn preload_hint_tag(&self, stream_prefix: &str) -> String;
}
```

### 2.6 Driver 层：Long-Poll 机制

**文件**: `crates/protocols/hls/driver-tokio/src/server.rs`

扩展 HTTP 服务器支持请求挂起：

```rust
/// 挂起的连接状态
struct PendingConnection {
    response_tx: oneshot::Sender<HttpResponseData>,
    cancel: CancellationToken,
    timeout_handle: JoinHandle<()>,
}
```

Driver 层行为：
1. 收到 blocking playlist 请求 → 解析 `_HLS_msn`/`_HLS_part` → 发送 `BlockingPlaylistRequested` 事件到 module
2. Module 判断数据是否就绪：
   - 就绪 → 立即通过 command 返回 playlist
   - 未就绪 → 注册到 `LowLatencyState` 的 pending 列表
3. 新 part 生成时 → module 检查 pending 列表 → 向 driver 发送响应 command
4. 超时 → driver 层 timeout task 触发 → 返回当前最新 playlist

### 2.7 Module 层：Blocking 请求编排

**文件**: `crates/protocols/hls/module/src/module.rs`

```rust
/// 每流的 blocking 请求管理
struct StreamBlockingState {
    pending_requests: Vec<PendingBlockingRequest>,
    max_pending_per_stream: usize,  // 防止连接泄漏
}

impl HlsModule {
    /// Part 就绪时通知所有等待的客户端
    fn notify_blocking_waiters(&mut self, stream_key: &str, msn: u64, part_idx: u64);

    /// 定时检查超时的 blocking 请求
    fn check_blocking_timeouts(&mut self);
}
```

---

## 3. 实现步骤

### Step 1: 请求路由扩展

1. `parse_hls_request()` 解析 `_HLS_msn`、`_HLS_part`、`_HLS_skip` 查询参数
2. 新增 `Part` 请求类型（URL 模式：`/{ns}/{stream}/part_<seq>.m4s`）
3. 单元测试：各种 URL + 查询参数组合的解析

### Step 2: Core 层 Blocking 模型

1. 实现 `PendingBlockingRequest` 注册/唤醒/超时逻辑
2. `on_part_ready()` 返回可唤醒的 connection_id 列表
3. `check_timeouts()` 返回超时的 connection_id 列表
4. 单元测试：注册→唤醒、注册→超时、立即满足

### Step 3: Delta Updates Playlist

1. 实现 `build_media_ll_delta()` — 生成含 `EXT-X-SKIP` 的增量 playlist
2. `CAN-SKIP-UNTIL` 值 = `segment_duration * (segment_count - 3)`
3. 属性测试：delta playlist 格式正确性

### Step 4: Preload Hint

1. 在 `build_media_ll()` 末尾追加 `EXT-X-PRELOAD-HINT` 标签
2. URI 指向下一个即将生成的 part
3. 单元测试：preload hint URI 正确

### Step 5: Driver 层 Long-Poll

1. 扩展 `ConnectionState` 支持挂起状态
2. Blocking 请求不立即响应，保持 `response_tx` 等待
3. 超时任务：`part_hold_back * 3` 后强制响应
4. 连接关闭时清理挂起状态
5. 集成测试：模拟 blocking 请求的完整流程

### Step 6: Module 层编排

1. `BlockingPlaylistRequested` 事件处理：检查 → 立即响应 or 注册等待
2. `MuxerOutput::PartReady` 时调用 `notify_blocking_waiters()`
3. 定时器驱动 `check_blocking_timeouts()`
4. 集成测试：端到端 blocking playlist reload

---

## 4. 非标准兼容特性

### 4.1 宽松的 `_HLS_msn` 验证

标准要求 `_HLS_msn` 不能超过当前 msn+1。实现中：
- `_HLS_msn` 超过当前 msn+2 时返回 400 Bad Request
- `_HLS_msn` 等于当前 msn+1 时正常 blocking（等待下一个 segment）
- `_HLS_msn` 小于 live window 起始时返回当前最新 playlist（不报错）

### 4.2 无 `_HLS_part` 时的行为

- 仅有 `_HLS_msn` 无 `_HLS_part` → 等待该 segment 的第一个 part 就绪
- 兼容不发送 `_HLS_part` 的旧版 hls.js

### 4.3 Blocking 超时策略

- 默认超时：`PART-HOLD-BACK * 6`（约 3.6 秒 @ 200ms part）
- 超时后返回当前最新 playlist（不返回错误）
- 防止客户端因超时重试风暴

### 4.4 并发 Blocking 请求限制

- 每流最多 100 个并发 blocking 请求
- 超过限制时拒绝新请求（返回 503）
- 防止恶意客户端耗尽服务器资源

### 4.5 Part 请求的 404 处理

- Part 已过期（被淘汰出缓冲）→ 返回 404
- Part 尚未生成 → 返回 404（不 blocking，仅 playlist 请求支持 blocking）
- 兼容某些播放器对 part 404 的重试逻辑

---

## 5. 测试计划

| 测试类型 | 范围 | 验证点 |
|----------|------|--------|
| 单元测试 | URL 解析 | `_HLS_msn`/`_HLS_part`/`_HLS_skip` 正确解析 |
| 单元测试 | Blocking 模型 | 注册/唤醒/超时/过期 |
| 属性测试 | Delta playlist | `EXT-X-SKIP` 格式、SKIPPED-SEGMENTS 计算 |
| 集成测试 | Long-poll 流程 | 请求挂起 → part 就绪 → 响应 |
| 集成测试 | 超时流程 | 请求挂起 → 超时 → 返回最新 |
| 压力测试 | 并发 blocking | 100 并发 blocking 请求正确处理 |

---

## 6. 验收标准

1. `_HLS_msn`/`_HLS_part` 参数正确触发 blocking 行为
2. 新 part 就绪时所有匹配的 blocking 请求被唤醒
3. 超时后返回当前最新 playlist，不返回错误
4. Delta updates 正确跳过已知 segments
5. Preload hint 指向正确的下一个 part URI
6. hls.js LLHLS 模式能正常播放（blocking + delta + preload）
7. 并发 blocking 请求不导致内存泄漏或连接泄漏
