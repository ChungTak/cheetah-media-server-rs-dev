# Phase 03 — CDN 兼容模式与非标准兼容特性

- **状态**: 待实现
- **前置**: Phase 02（Blocking Playlist Reload 与 Part 端点）
- **目标**: 实现 CDN 分发兼容、多种会话标识模式、iOS 设备兼容、以及其他提高鲁棒性的非标准特性
- **mediamtx 参考**: `http_server.go` CDN 认证 + session 管理 + iOS 检测 + `.mp` 后缀兼容

---

## 1. 需求分析

### 1.1 mediamtx CDN 模式

mediamtx 通过 `hlsCDNSecret` 配置实现 CDN 兼容：
- CDN 边缘节点通过 `Authorization: Bearer <secret>` 认证
- CDN 请求不设置 `Cache-Control: no-cache`（允许边缘缓存）
- CDN session 不绑定 IP（边缘节点 IP 会轮换）
- 每个 muxer 只有一个 CDN session

### 1.2 Session 双模式

mediamtx 支持两种 session 标识方式：
- **Cookie 模式**: `HLS_SESSION=<secret>` cookie
- **Query Param 模式**: `?session=<secret>` URL 参数
- 兼容不支持 cookie 的播放器（如某些嵌入式设备）

### 1.3 iOS 兼容

- 检测 iOS User-Agent
- iOS 原生 HLS 播放器要求 cookie 支持
- 首次请求重定向设置 cookie，验证 cookie 可用后创建 session

### 1.4 `.mp` 后缀兼容

- 某些 CDN 对 `.mp4` 扩展名有特殊处理（如强制 range request）
- mediamtx 支持 `.mp` 后缀，内部自动补全为 `.mp4`

---

## 2. 设计方案

### 2.1 CDN 模式设计

**配置项**:

```rust
pub struct HlsModuleConfig {
    // 已有字段...

    /// CDN Bearer token 密钥，为空则禁用 CDN 模式
    pub cdn_secret: Option<String>,

    /// CDN 模式下的 Cache-Control 策略
    pub cdn_cache_control: CdnCacheControl,
}

pub enum CdnCacheControl {
    /// 不设置 Cache-Control（允许 CDN 缓存）
    None,
    /// 设置 max-age（秒）
    MaxAge(u32),
    /// 自定义 Cache-Control 值
    Custom(String),
}
```

**认证流程**:

```
CDN 边缘节点 → 服务器
  GET /live/test/index.m3u8
  Authorization: Bearer <cdn_secret>

服务器验证 Bearer token:
  - 匹配 → CDN session（不绑定 IP，不设 no-cache）
  - 不匹配 → 401 Unauthorized
```

**Cache-Control 策略**:

| 请求来源 | Playlist | Segment/Part | Init Segment |
|----------|----------|--------------|--------------|
| 普通客户端 | `no-cache, no-store` | `max-age=86400` | `max-age=86400` |
| CDN 边缘 | 不设置 / 自定义 | `max-age=86400` | `max-age=86400` |

### 2.2 Session 双模式设计

**文件**: `crates/protocols/hls/module/src/module.rs`

```rust
/// Session 标识来源
enum SessionSource {
    Cookie(String),       // HLS_SESSION cookie
    QueryParam(String),   // ?session=<secret>
    CdnBearer(String),    // Authorization: Bearer <secret>
}

impl HlsModule {
    /// 从请求中提取 session 标识
    fn extract_session_id(&self, request: &HttpRequest) -> Option<SessionSource>;
}
```

Session 查找优先级：
1. `Authorization: Bearer` → CDN session
2. `?session=` query param → 普通 session
3. `HLS_SESSION` cookie → 普通 session

### 2.3 iOS 兼容设计

**文件**: `crates/protocols/hls/driver-tokio/src/server.rs`

```rust
/// iOS UA 检测
fn is_ios_user_agent(ua: &str) -> bool {
    ua.contains("iPhone") || ua.contains("iPad") || ua.contains("iPod")
        || (ua.contains("Mac OS") && ua.contains("Safari") && !ua.contains("Chrome"))
}
```

iOS 兼容流程：
1. 首次请求 `index.m3u8` → 检测 UA
2. 如果是 iOS 且无 cookie → 302 重定向到 `?_cookie_check=1`
3. 重定向响应设置 `Set-Cookie: HLS_SESSION=<new_secret>`
4. 客户端带 cookie 重新请求 → 创建 session → 返回 playlist
5. 非 iOS 客户端 → 直接创建 session（cookie + query param 双模式）

### 2.4 `.mp` 后缀兼容

**文件**: `crates/protocols/hls/core/src/request.rs`

```rust
// URL 解析时处理 .mp 后缀
fn normalize_extension(path: &str) -> &str {
    if path.ends_with(".mp") {
        // 内部当作 .mp4 处理
        &path[..path.len() - 3]  // 去掉 .mp，后续按 init segment 处理
    } else {
        path
    }
}
```

### 2.5 AlwaysRemux 模式增强

当前 `hls_demand` 配置控制按需生成。增强为：

```rust
pub enum HlsMuxerMode {
    /// 按需模式：有观众时才启动 muxer（现有 hls_demand=true）
    OnDemand,
    /// 始终模式：流就绪时自动启动 muxer（对应 mediamtx hlsAlwaysRemux）
    Always,
    /// 混合模式：首次请求后保持 muxer 活跃一段时间
    Hybrid { keep_alive_secs: u64 },
}
```

---

## 3. 实现步骤

### Step 1: CDN Bearer Token 认证

1. 配置项 `cdn_secret` 解析
2. HTTP 请求中提取 `Authorization: Bearer` header
3. Token 验证逻辑
4. CDN session 创建（不绑定 IP）
5. 单元测试：认证成功/失败/缺失

### Step 2: Cache-Control 策略

1. 普通请求：playlist 设置 `no-cache`，segment 设置 `max-age`
2. CDN 请求：playlist 不设置或自定义，segment 设置 `max-age`
3. Init segment：始终设置长 `max-age`（内容不变）
4. Part：设置 `max-age=<part_target * 2>`（短期缓存）

### Step 3: Session Query Param 模式

1. `parse_hls_request()` 解析 `?session=` 参数
2. Session 查找支持 cookie 和 query param 双来源
3. Master playlist 响应中包含 `?session=<secret>` 在 media playlist URL 中
4. 兼容测试：无 cookie 环境下通过 query param 维持会话

### Step 4: iOS UA 检测与 Cookie 验证

1. Driver 层 UA 解析
2. iOS 首次请求重定向流程
3. Cookie 验证后创建 session
4. 非 iOS 设备跳过重定向
5. 集成测试：模拟 iOS UA 的完整流程

### Step 5: `.mp` 后缀兼容

1. URL 解析层处理 `.mp` → `.mp4` 映射
2. Playlist 中 init segment URI 可配置使用 `.mp` 后缀
3. 单元测试：`.mp` URL 正确路由

### Step 6: Muxer 模式增强

1. `HlsMuxerMode` 枚举替代布尔 `hls_demand`
2. `Always` 模式：流注册时自动创建 muxer
3. `Hybrid` 模式：首次请求后保持 muxer 活跃
4. 配置兼容：旧 `hls_demand: true` 映射为 `OnDemand`

---

## 4. 非标准兼容特性

### 4.1 宽松的 Session 过期策略

- 标准无 session 概念，但实际部署需要
- Session 过期时间可配置（默认 30s 无请求过期）
- CDN session 永不过期（CDN 边缘节点间隔可能较长）
- Session 过期后首次请求自动创建新 session（不返回错误）

### 4.2 多 CDN 节点支持

- 允许配置多个 CDN secret（逗号分隔）
- 支持 CDN 密钥轮换（新旧密钥同时有效）
- CDN session 按 secret 区分，支持多个 CDN 同时拉取

### 4.3 CORS 增强

- LLHLS 跨域请求需要 `Access-Control-Expose-Headers` 包含自定义头
- 预检请求缓存时间可配置
- 支持 `Access-Control-Allow-Credentials: true`（cookie 跨域）

### 4.4 ETag 与条件请求

- Playlist ETag 基于 msn + part_idx 生成
- 支持 `If-None-Match` 条件请求（304 Not Modified）
- 减少不必要的 playlist 传输

### 4.5 Playlist URL 中的 Session 传递

- Master playlist 中的 media playlist URL 自动附加 `?session=<secret>`
- 兼容不转发 cookie 的 CDN 代理
- 可配置是否在 URL 中暴露 session（安全考虑）

### 4.6 请求频率限制

- 同一 session 的 playlist 请求频率限制（防止轮询风暴）
- 非 blocking 请求：最小间隔 = `part_target / 2`
- 超频请求返回 429 Too Many Requests

---

## 5. 测试计划

| 测试类型 | 范围 | 验证点 |
|----------|------|--------|
| 单元测试 | CDN 认证 | Bearer token 验证正确性 |
| 单元测试 | Session 提取 | Cookie/QueryParam/Bearer 优先级 |
| 单元测试 | iOS UA 检测 | 各种 UA 字符串正确识别 |
| 单元测试 | `.mp` 后缀 | URL 路由正确映射 |
| 集成测试 | CDN 完整流程 | 认证 → session → playlist → segment |
| 集成测试 | iOS 重定向 | 首次请求 → 重定向 → cookie → session |
| 集成测试 | 无 cookie 播放 | query param 模式端到端 |
| 安全测试 | Session 伪造 | 无效 session 正确拒绝 |

---

## 6. 验收标准

1. CDN Bearer token 认证正确工作
2. CDN 请求不设置 `no-cache`，普通请求设置 `no-cache`
3. 无 cookie 环境通过 query param 正常播放
4. iOS Safari 通过 cookie 重定向流程正常播放
5. `.mp` 后缀 URL 正确路由到 init segment
6. Session 过期后自动重建，不中断播放
7. 多 CDN 节点同时拉取不冲突
