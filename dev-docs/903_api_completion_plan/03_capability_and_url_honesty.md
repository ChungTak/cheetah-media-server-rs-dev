# 03 · 能力与 URL 真实性

## 1. 目标模型

保留 `MediaCapabilitySet` 作为兼容摘要，只列 `Available` 或明确允许使用的 `Degraded` 能力。新增：

```rust
struct MediaCapabilityReport {
    generation: u64,
    descriptors: Vec<MediaCapabilityDescriptor>,
}

struct MediaCapabilityDescriptor {
    capability: MediaCapability,
    version: u32,
    provider_id: String,
    state: CapabilityState,
    operations: Vec<String>,
    reason: Option<String>,
}
```

descriptor 按 capability、provider_id 排序；generation 在 provider、state、operations 或输出端点变化时单调递增。`Degraded` 必须带 reason，并只列仍可调用的 operations。

## 2. 输出端点注册

`MediaOutputRegistryApi` 管理 `MediaOutputEndpoint`：`registration_id`、provider、`MediaSchema`、public host、port、TLS、path template、state。RTMP、RTSP、HTTP-FLV、HLS、WebRTC 等 module 在 start 成功后注册，在 stop/restart/drop 时注销。注册不能使用请求 Host 头。

URL resolver 的规则固定为：

1. 读取 endpoint snapshot 和 media online state。
2. 空 schemas 返回所有 active endpoints；指定 schema 不活跃时返回 `Unsupported`。
3. 根据配置 public origin 和模板编码 vhost/app/stream，禁止字符串直接拼接未转义路径。
4. endpoint 不可用时不生成 URL；媒体离线可返回 URL 但 `available=false`，仅当 schema endpoint 活跃。
5. 需要签名时使用 HMAC-SHA256，签名覆盖 schema、authority、path、expiry 和 key id；常量时间比较，过期拒绝。

## 3. HTTP 契约

- `GET /api/v1/media/capabilities`：保留 set 兼容响应，但内容改为真实可用集合。
- `GET /api/v1/media/capabilities/details`：返回 report；需要 `media.read`。
- `GET /api/v1/media/{encoded_key}/urls?schema=...`：返回已注册端点 URL；未知 schema 为 400，未运行 schema 为 501/Unsupported。

不得通过编译 feature 单独判定 Available；feature 只说明代码可存在，module 生命周期和 provider health 才决定运行状态。

## 4. 任务与测试

- `CAP-01`：实现 report 聚合与单调 generation。
- `CAP-02`：实现 output registry 和各协议 module 生命周期接线。
- `CAP-03`：resolver 改为 registry 驱动。
- `CAP-04`：替换自定义签名并增加 key rotation 配置。

测试必须覆盖 provider 注册/替换/注销、module 重启、degraded operations、schema 子集、恶意 Host、路径转义、签名篡改/过期/旧密钥，以及“未启动 RTSP 时绝不返回 RTSP URL”。

完成标准：能力详情、摘要、URL、实际监听端口和可建立连接的协议完全一致。
