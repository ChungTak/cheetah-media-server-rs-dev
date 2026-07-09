# Phase 04 — PROGRAM-DATE-TIME、Rendition Report 与 HTTP/2

- **状态**: 待实现
- **前置**: Phase 03（CDN 兼容模式）
- **目标**: 实现 LLHLS 高级特性——绝对时间戳同步、多 rendition 报告、HTTP/2 多路复用，进一步降低延迟并提升播放器兼容性
- **mediamtx 参考**: gohlslib NTP 时间戳 + `to_stream.go` 绝对时间处理

---

## 1. 需求分析

### 1.1 EXT-X-PROGRAM-DATE-TIME

LLHLS 规范推荐在 playlist 中包含绝对时间戳：
```
#EXT-X-PROGRAM-DATE-TIME:2026-05-16T01:02:03.456Z
#EXTINF:4.0,
seg10.m4s
```

用途：
- 播放器同步多 rendition 的播放位置
- DVR 回放定位
- 广告插入时间对齐
- 多源切换时的无缝衔接

mediamtx 通过 `ntpestimator` 处理：
- 源提供 NTP 时间 → 直接使用
- 源不提供 → 使用本地时钟估算
- 可配置是否使用源的绝对时间

### 1.2 EXT-X-RENDITION-REPORT

多码率场景下，每个 rendition 的 playlist 中报告其他 rendition 的最新状态：
```
#EXT-X-RENDITION-REPORT:URI="../720p/index.m3u8",LAST-MSN=15,LAST-PART=2
#EXT-X-RENDITION-REPORT:URI="../360p/index.m3u8",LAST-MSN=15,LAST-PART=1
```

用途：
- 客户端切换码率时无需额外请求即可知道目标 rendition 的最新位置
- 减少码率切换延迟

### 1.3 HTTP/2

Apple LLHLS 规范强烈推荐 HTTP/2：
- 多路复用：playlist + part 请求共享连接
- Server Push（已废弃，但 HTTP/2 连接复用仍有价值）
- 头部压缩：减少重复 header 开销
- 对 HTTPS 场景（iOS 要求）尤为重要

---

## 2. 设计方案

### 2.1 PROGRAM-DATE-TIME 实现

**Core 层**:

```rust
/// 时间戳来源
pub enum ProgramDateTimeSource {
    /// 使用源流提供的绝对时间（如 RTMP onMetaData 中的 NTP）
    SourceNtp,
    /// 使用服务器本地时钟
    LocalClock,
    /// 禁用
    Disabled,
}

/// Segment 扩展绝对时间信息
pub struct Segment {
    // 已有字段...
    pub program_date_time: Option<DateTime>,  // 新增
}

/// 简单的日期时间表示（不依赖 chrono）
pub struct DateTime {
    pub timestamp_ms: i64,  // Unix 毫秒时间戳
}

impl DateTime {
    pub fn to_iso8601(&self) -> String;  // 格式化为 ISO 8601
}
```

**Playlist 生成**:

```rust
impl PlaylistBuilder {
    /// 在 segment 前插入 EXT-X-PROGRAM-DATE-TIME
    /// 规则：第一个 segment 必须有，之后每个 segment 都有（LLHLS 推荐）
    fn format_program_date_time(dt: &DateTime) -> String {
        format!("#EXT-X-PROGRAM-DATE-TIME:{}", dt.to_iso8601())
    }
}
```

**Module 层时间注入**:

```rust
impl StreamMuxer {
    /// 设置时间基准（流开始时调用）
    pub fn set_time_base(&mut self, source: ProgramDateTimeSource, base_ntp_ms: Option<i64>);

    /// 每个 segment 开始时计算绝对时间
    fn compute_program_date_time(&self, segment_start_dts_ms: i64) -> Option<DateTime>;
}
```

### 2.2 Rendition Report 实现

**Core 层**:

```rust
/// 其他 rendition 的状态报告
pub struct RenditionReport {
    pub uri: String,        // 相对 URI
    pub last_msn: u64,      // 最新 segment 序号
    pub last_part: Option<u64>,  // 最新 part 序号
}

impl LowLatencyState {
    /// 生成 EXT-X-RENDITION-REPORT 标签
    pub fn rendition_report_tags(&self, reports: &[RenditionReport]) -> String;
}
```

**Module 层**:

```rust
/// 多码率流的 rendition 状态同步
struct RenditionStateRegistry {
    /// stream_key → (last_msn, last_part)
    states: HashMap<String, (u64, Option<u64>)>,
}

impl HlsModule {
    /// Part 就绪时更新 rendition 状态
    fn update_rendition_state(&mut self, stream_key: &str, msn: u64, part: u64);

    /// 生成指定流的 rendition reports（排除自身）
    fn get_rendition_reports(&self, stream_key: &str) -> Vec<RenditionReport>;
}
```

### 2.3 HTTP/2 支持

**Driver 层**:

HTTP/2 支持方案选择：
- **方案 A**: 使用 `hyper` 替代自定义 HTTP 解析器（大改动）
- **方案 B**: 使用 `h2` crate 在现有架构上叠加 HTTP/2（中等改动）
- **方案 C**: 仅在 TLS 场景通过 ALPN 协商 HTTP/2（最小改动）

**推荐方案 C**（最小改动，最大收益）：

```rust
pub struct HlsDriverConfig {
    // 已有字段...

    /// 启用 HTTP/2（仅 TLS 模式下生效）
    pub enable_h2: bool,
}
```

实现策略：
1. TLS 握手时通过 ALPN 协商 `h2` 或 `http/1.1`
2. 协商为 `h2` 时使用 `h2` crate 处理帧
3. 协商为 `http/1.1` 时走现有路径
4. 非 TLS 连接始终使用 HTTP/1.1（HTTP/2 cleartext 不常用）

```rust
// TLS ALPN 配置
let mut tls_config = rustls::ServerConfig::builder()
    .with_no_client_auth()
    .with_single_cert(certs, key)?;
tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
```

---

## 3. 实现步骤

### Step 1: PROGRAM-DATE-TIME 基础

1. `DateTime` 结构体和 ISO 8601 格式化
2. `Segment` 新增 `program_date_time` 字段
3. `PlaylistBuilder` 在 segment 前输出 `EXT-X-PROGRAM-DATE-TIME`
4. 单元测试：时间格式化正确性

### Step 2: 时间注入机制

1. `StreamMuxer::set_time_base()` — 流开始时设置时间基准
2. `compute_program_date_time()` — 基于 DTS 偏移计算绝对时间
3. `LocalClock` 模式：使用 segment 创建时的系统时间
4. `SourceNtp` 模式：使用源流提供的 NTP 时间戳
5. Module 层在流订阅时注入时间基准

### Step 3: Rendition Report

1. `RenditionStateRegistry` 实现
2. Part 就绪时更新 registry
3. Playlist 生成时查询其他 rendition 状态
4. `EXT-X-RENDITION-REPORT` 标签生成
5. 仅在多码率配置（`master_playlists` 非空）时启用

### Step 4: HTTP/2 ALPN 协商

1. TLS 配置添加 ALPN `h2` + `http/1.1`
2. 连接建立后检查协商结果
3. HTTP/2 连接使用 `h2` crate 处理
4. 请求/响应映射到现有 event/command 模型

### Step 5: HTTP/2 请求处理

1. `h2` stream → `HlsDriverEvent::Core` 映射
2. 响应通过 `h2` stream 发送
3. 多路复用：同一连接的多个请求并行处理
4. Flow control 与背压集成

---

## 4. 非标准兼容特性

### 4.1 PROGRAM-DATE-TIME 精度容忍

- 标准要求毫秒精度，但某些源只提供秒精度
- 实现中：秒精度时补 `.000`，不报错
- 时间戳跳变（如源重启）时重新校准，不中断播放

### 4.2 时间戳回绕处理

- 源流 DTS 回绕时（33-bit wrap），绝对时间继续递增
- 检测回绕条件：DTS 突然减小超过 `2^32 / 2`
- 回绕后重新计算时间偏移

### 4.3 Rendition Report 降级

- 当其他 rendition 的 muxer 尚未就绪时，省略该 rendition 的 report
- 不因某个 rendition 异常影响其他 rendition 的 playlist 生成

### 4.4 HTTP/2 降级

- 客户端不支持 HTTP/2 时自动降级到 HTTP/1.1
- HTTP/2 连接异常时不影响 LLHLS 功能（仅影响性能）
- 非 TLS 场景不尝试 HTTP/2（避免兼容问题）

### 4.5 PROGRAM-DATE-TIME 与 Part 对齐

- 每个 segment 的第一个 part 携带 PROGRAM-DATE-TIME
- Part 级别的时间精度：基于 segment 时间 + part 内偏移计算
- 兼容不解析 part 级别时间的播放器

---

## 5. 测试计划

| 测试类型 | 范围 | 验证点 |
|----------|------|--------|
| 单元测试 | DateTime 格式化 | ISO 8601 输出正确 |
| 单元测试 | 时间计算 | DTS → 绝对时间映射正确 |
| 单元测试 | Rendition Report | 标签格式、多 rendition 正确 |
| 属性测试 | Playlist 完整性 | 含 PDT + Rendition Report 的 playlist 格式正确 |
| 集成测试 | HTTP/2 协商 | ALPN 协商 + 请求处理 |
| 集成测试 | 多码率场景 | Rendition Report 跨流同步 |
| 兼容测试 | Safari LLHLS | iOS Safari 播放验证 |

---

## 6. 验收标准

1. Playlist 包含正确的 `EXT-X-PROGRAM-DATE-TIME` 标签
2. 时间戳单调递增，不因源流异常而跳变
3. 多码率场景下 Rendition Report 正确反映各 rendition 状态
4. HTTPS 连接成功协商 HTTP/2
5. HTTP/2 多路复用正常工作（多个并发请求共享连接）
6. HTTP/1.1 客户端不受 HTTP/2 功能影响
7. Safari + hls.js 均能正确解析 PROGRAM-DATE-TIME
