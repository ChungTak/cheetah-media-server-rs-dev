# 05 · Gap 3：HTTP-FLV Streaming `SubscriberSource`

> **Agent 用途**：阶段 2 主文档——将 one-shot pull 扩展为长生命周期 streaming 源。  
> **现状入口**：`crates/protocols/http-flv/module/src/pull.rs`。  
> **契约对齐**：`cheetah_sdk::SubscriberSource`。

---

## 1. 目标

| 项 | 首版（P0） | 后置 |
| --- | --- | --- |
| API | `open_http_flv_subscriber` → 可 `recv` 的 source | WS/HTTP 细粒度调优 |
| 映射 | FLV A/V tag → `AVFrame` + tracks | 全 codec 矩阵 |
| 生命周期 | cancel / close 终态明确 | 自动 failover 多 URL |
| 队列 | bounded + 策略 | 动态扩容（默认不要） |
| 兼容 | **保留** `pull_http_flv_once` | — |

---

## 2. 落点分层

| 组件 | 职责 |
| --- | --- |
| `cheetah-http-flv-module` | **实现** streaming pull（连接、读 body、demux、映射、队列） |
| `cheetah-codec` | FLV demux / 已有 frame 映射工具（**复用，不复制**） |
| `cheetah-connector` | 统一 `open_pull(Protocol::HttpFlv, …)` 包装 |

**不要** 只在 connector 里堆 1000 行 HTTP 客户端逻辑；module 是协议接入层。

---

## 3. Public API（proposed）

### 3.1 module 层

```rust
// crates/protocols/http-flv/module/src/streaming.rs（新建）

use std::sync::Arc;
use cheetah_codec::AVFrame;
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use cheetah_sdk::{SubscriberId, SubscriberSource}; // 若 module 已依赖 sdk

#[derive(Debug, Clone)]
pub struct HttpFlvSubscriberOptions {
    pub read_limits: PullReadLimits,
    /// 有界队列容量（帧数）
    pub queue_capacity: usize,
    /// 重连：None = 不重连；Some 定义 backoff
    pub reconnect: Option<ReconnectPolicy>,
    /// 媒体过滤
    pub enable_video: bool,
    pub enable_audio: bool,
}

#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    pub max_attempts: u32,          // 0 = 无限（需谨慎；测试用有限）
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub multiplier: f64,            // 或固定 enum，避免浮点争议：用 u32 分子分母
}

/// Open a long-lived HTTP/WS FLV pull as a frame source.
pub async fn open_http_flv_subscriber(
    runtime_api: Arc<dyn RuntimeApi>,
    source_url: &str,
    options: HttpFlvSubscriberOptions,
    cancel: CancellationToken,
) -> Result<HttpFlvSubscriber, HttpFlvPullError>;

pub struct HttpFlvSubscriber { /* internal */ }

impl HttpFlvSubscriber {
    pub fn id(&self) -> SubscriberId;
    pub fn tracks(&self) -> Vec<TrackInfo>; // 随 bootstrap 更新
    pub async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, HttpFlvPullError>;
    pub async fn close(&mut self) -> Result<(), HttpFlvPullError>;
}
```

实现 `SubscriberSource`（推荐）：

```rust
#[async_trait]
impl SubscriberSource for HttpFlvSubscriber {
    async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, SdkError> {
        self.recv().await.map_err(|e| /* map to SdkError 或保留扩展 */)
    }
    async fn close(&mut self) -> Result<(), SdkError> { … }
    fn id(&self) -> SubscriberId { … }
}
```

> 若坚持 `HttpFlvPullError` 比 `SdkError` 更丰富，可 **不** 直接 impl trait，而由 connector 包装成 `PullHandle` 并映射到 `ConnectorError`。两种均可；**推荐 connector 包装**，module 保留细错误。

### 3.2 connector 层

```rust
// open_pull(Protocol::HttpFlv, url, opts) → PullHandle
// 内部调用 open_http_flv_subscriber
```

---

## 4. 数据路径

```text
TCP/TLS/WS connect
  → read headers / status
  → body bytes
  → FlvDemuxer (cheetah-codec)
  → FlvTag stream
  → map_tag_to_avframe + track bootstrap
  → bounded channel (queue_capacity)
  → recv() 侧
```

### 4.1 Tag → AVFrame 映射要求

对每个输出帧，尽量填充：

| 字段 | 要求 |
| --- | --- |
| `track_id` | 音视频分离稳定 id |
| `media_kind` | Audio / Video |
| `codec` | 从 sequence header / codec id 解析，禁止默认 Unknown（除非真未知） |
| `format` | 与 codec 一致的 bitstream 视图 |
| `pts`/`dts`/`timebase` | FLV timestamp 映射；timebase 文档钉死（常见 1ms） |
| `pts_us`/`dts_us` | 与 timebase 一致 |
| `flags` | keyframe / config 等 |
| `payload` | ES 或协议约定格式（与 codec 导出一致） |
| `origin` | 建议 `Ingest` 或协议专用 origin（若有） |

`TrackInfo` / `CodecExtradata`：在 sequence header / metadata tag 到达时 `update`；`tracks()` 可读。

复用现有映射：

```bash
rg -n 'FlvTag|sequence header|AVC|AAC|map_.*flv|FlvDemux' \
  crates/foundation/cheetah-codec crates/protocols/http-flv --glob '*.rs' | head -60
```

### 4.2 队列与背压

| 规则 | 说明 |
| --- | --- |
| 容量 | `queue_capacity` 必须有限，默认建议 64–150（对齐 sdk 默认） |
| 满 | 定义策略：`DropOldest` / `DropUntilKeyframe` / `Block`（选一种默认并文档化） |
| 禁止 | 无界 `Vec` 堆积到 OOM |

读取任务与 `recv` 解耦：后台 task 用 `RuntimeApi::spawn`（注意 module 不直接 `tokio::spawn`）。

### 4.3 重连

| 条件 | 行为 |
| --- | --- |
| `reconnect = None` | 读结束或错误 → `recv` 返回 `Ok(None)` 或 `Err`（钉死一种；推荐可重试错误 Err，干净 EOF Ok(None)） |
| `reconnect = Some` | 仅对 retryable 错误重连；耗尽 attempts 后终态错误 |
| cancel | 立即停止；`recv` 返回 `Cancelled` 映射 |

将 `HttpFlvPullError::retryable` 提升为 `pub`（若仍私有）。

### 4.4 取消与关闭

```text
cancel fired     → 读循环退出 → channel close → recv → Ok(None) 或 Err(Cancelled)
close() called   → 同样清理；幂等
drop subscriber  → 应 cancel 后台任务（防止泄漏）
```

---

## 5. 与 one-shot API 关系

| API | 用途 |
| --- | --- |
| `pull_http_flv_once` | 脚本、抓包、属性测试、短文件 |
| `open_http_flv_subscriber` | 长连接播放、connector、CI loopback |

共享：URL 解析、HTTP/WS 握手、`PullReadLimits`、demux。  
**抽取** 公共内部函数，避免复制粘贴分叉。

建议内部结构：

```text
pull.rs          # one-shot 保留
streaming.rs     # 新 API
io_common.rs     # 可选：连接与读 body 共享
map.rs           # tag→frame（若尚无独立模块）
```

---

## 6. 测试清单

| ID | 用例 | 期望 |
| --- | --- | --- |
| T-HF-01 | 本地 HTTP-FLV fixture 或 loopback server 逐帧 recv | ≥1 video frame |
| T-HF-02 | cancel 后 recv 终态 | 不挂死 |
| T-HF-03 | close 幂等 | 第二次 Ok |
| T-HF-04 | queue 满策略 | 不 OOM；可观测 drop 或阻塞有超时 |
| T-HF-05 | 错误 URL | `InvalidUrl` |
| T-HF-06 | Bad status | typed 错误 |
| T-HF-07 | reconnect 有限次 | 达到 max_attempts 后失败 |
| T-HF-08 | one-shot 回归 | 既有测试仍绿 |

本地 server：复用 module 测试中的 HTTP-FLV 播放路径；或 `hyper`/engine 路由。

---

## 7. DoD（阶段 2）

- [ ] `open_http_flv_subscriber` 可编译可测  
- [ ] 逐帧 `recv` 产出 `AVFrame`（非仅 `FlvTag`）  
- [ ] cancel/close/bounded queue 有测试  
- [ ] one-shot API 保留且回归通过  
- [ ] connector `open_pull(HttpFlv)` 可调用（若 S1 已合）  
- [ ] `cargo test -p cheetah-http-flv-module` 相关测通过  

---

## 8. 非目标

- 完整浏览器 MSE 兼容矩阵。  
- 在 pull 路径做转码。  
- 替换 RTMP/HTTP-FLV 播放服务端实现。  
