# Phase 01 — Blocking Delivery（阻塞式交付）

- **目标**: 实现完整的 Blocking Playlist Reload 和 Partial Segment Blocking，使 LLHLS 播放器（hls.js lowLatencyMode）能正常工作
- **前置**: 本地已有 `BlockingPlaylistRequested` 事件、`_HLS_msn`/`_HLS_part` 解析、Part HTTP 端点
- **OME 参考**: `llhls_session.cpp::OnPlaylistUpdated` / `AddPendingRequest` / `ResponseChunklist`

---

## 1. Blocking Playlist Reload

### 1.1 原理（OME 实现模式）

```
Player → GET /chunklist.m3u8?_HLS_msn=5&_HLS_part=2
Server → 检查当前 last_msn/last_part
         如果 (msn > last_msn) || (msn == last_msn && part > last_part)
           → 将请求加入 pending_requests 列表（hold 住 HTTP 连接）
           → 等待 OnPlaylistUpdated 通知
         否则
           → 立即返回当前 playlist
```

### 1.2 实现方案

**core 层（已完成）**:
- `BlockingPlaylistRequested` 事件已定义，包含 `connection_id`、`blocking: BlockingParams`、`skip`

**module 层（新增）**:
- `StreamMuxer` 新增 `pending_playlist_requests: Vec<PendingPlaylistRequest>` 字段
- `PendingPlaylistRequest` 结构体：
  ```rust
  struct PendingPlaylistRequest {
      connection_id: u64,
      target_msn: u64,
      target_part: Option<u64>,
      skip: Option<SkipMode>,
      session_id: Option<u64>,
      created_at: Instant,  // 注入的时间
  }
  ```
- 收到 `BlockingPlaylistRequested` 时：
  - 比较 target_msn/target_part 与当前 `ll_state.parent_segment_seq()`/`ll_state.next_part_seq()`
  - 如果当前已满足 → 立即构建 playlist 返回
  - 否则 → 加入 pending 列表
- 每次 `PartReady` 或 `SegmentReady` 事件后：
  - 遍历 pending_requests，检查是否满足
  - 满足的 → 构建 playlist，通过 output 通道发回 driver
  - 不满足的 → 保留

**driver 层（新增）**:
- `server.rs` 收到 `BlockingPlaylistRequested` 事件后，不立即返回 HTTP 响应
- 使用 `tokio::sync::oneshot` channel：创建 (tx, rx) 对
  - tx 发给 module（通过 event channel）
  - 在 HTTP handler 中 `tokio::select!` 等待 rx 或超时
- 超时（默认 30s）返回 HTTP 200 + 当前 playlist（不是 408）
- module 侧通过 `PlaylistResponse { connection_id, content }` 命令触发 tx.send()

### 1.3 满足条件判定逻辑（对标 OME）

```rust
fn is_blocking_satisfied(&self, target_msn: u64, target_part: Option<u64>) -> bool {
    let current_msn = self.ll_state.parent_segment_seq();
    let current_part = self.ll_state.next_part_seq().saturating_sub(1);

    match target_part {
        Some(tp) => current_msn > target_msn || (current_msn == target_msn && current_part >= tp),
        None => current_msn >= target_msn,
    }
}
```

---

## 2. Partial Segment Blocking

### 2.1 原理（OME 实现模式）

```
Player → GET /part_3_5_2.m4s  (track=3, segment=5, partial=2)
Server → 检查该 partial 是否已生成
         如果未生成 → hold 请求，等待 OnMediaChunkUpdated
         如果已生成 → 立即返回 part data
```

### 2.2 实现方案

**module 层**:
- `StreamMuxer` 新增 `pending_part_requests: Vec<PendingPartRequest>`
- `PendingPartRequest` 结构体：
  ```rust
  struct PendingPartRequest {
      connection_id: u64,
      target_part_seq: u64,
      created_at: Instant,
  }
  ```
- 收到 `PartRequested` 事件时：
  - 解析 part 序号，查找 `ll_state.get_part(seq)`
  - 如果已有 → 立即返回
  - 如果没有且 seq == next_part_seq → 加入 pending
  - 如果没有且 seq > next_part_seq + 合理范围 → 返回 404
- 每次 `PartReady` 后遍历 pending_part_requests 释放

**driver 层**:
- 同 playlist blocking：oneshot channel + select + 超时

---

## 3. Pending 请求上限保护

### 3.1 配置项

```yaml
hls:
  max_pending_requests: 10      # 每个 stream 最大 pending 请求数
  blocking_timeout_ms: 30000    # blocking 请求超时（ms）
```

### 3.2 实现

- pending_playlist_requests 和 pending_part_requests 合计不超过 `max_pending_requests`
- 超过时：拒绝新 blocking 请求，立即返回当前 playlist（降级为非 blocking）
- 超时清理：module 层定时器（或 driver 侧 select! 超时）清理过期 pending

---

## 4. 连接断开清理

- driver 层检测连接断开时，发送 `ConnectionClosed { connection_id }` 事件到 module
- module 层从 pending 列表中移除对应 connection_id 的请求
- 避免 oneshot tx 发送到已关闭连接（tx.send() 返回 Err 时忽略）

---

## 5. 涉及文件变更

| 层 | 文件 | 变更 |
|----|------|------|
| core | `session.rs` | 无变更（事件已定义） |
| core | `request.rs` | 无变更（解析已实现） |
| module | `muxer.rs` | 新增 pending_playlist_requests / pending_part_requests + release 逻辑 |
| module | `module.rs` | 新增 blocking 请求路由 + PartReady/SegmentReady 后释放 pending |
| driver | `server.rs` | blocking 请求改用 oneshot channel + select! 超时 |
| module | `config.rs` | 新增 max_pending_requests / blocking_timeout_ms 配置 |

---

## 6. 测试计划

| 测试 | 层 | 方法 |
|------|-----|------|
| blocking 满足条件判定 | core/module | 单元测试：构造不同 msn/part 组合验证 is_blocking_satisfied |
| pending 请求释放 | module | 单元测试：模拟 PartReady 事件后 pending 列表清空 |
| 超时降级 | driver | 集成测试：发送 blocking 请求，不产生新 part，验证超时返回 |
| pending 上限 | module | 单元测试：超过 max_pending 后请求立即返回 |
| 连接断开清理 | module | 单元测试：模拟 ConnectionClosed 后 pending 移除 |
| hls.js 端到端 | e2e | 推流 → hls.js lowLatencyMode 播放验证 |

---

## 7. 完成标准

- [x] `_HLS_msn` / `_HLS_part` 请求正确 hold 住 HTTP 连接
- [x] 新 part/segment 生成后，pending 请求立即释放并返回最新 playlist
- [x] 超时 30s 后返回当前 playlist（不阻塞 forever）
- [x] pending 请求数超过上限时降级为立即响应
- [x] 连接断开时清理 pending
- [x] hls.js `lowLatencyMode: true` 端到端播放正常
- [x] `cargo test -p cheetah-hls-core -- ll_hls` 通过
- [x] `cargo test -p cheetah-hls-module -- llhls` 通过
