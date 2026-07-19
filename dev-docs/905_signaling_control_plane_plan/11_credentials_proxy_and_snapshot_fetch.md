# 11 · 凭据、Proxy 与受限 Snapshot Fetch

## 1. CRED-01：公共凭据合同

`PullProxyRequest` 增加 `credential_handle: Option<CredentialHandle>`，`source_url` 必须是
sanitized URL。新增 runtime-neutral：

```text
CredentialExchangeApi::exchange(context, handle, purpose, resource_ref)
  -> CredentialLease
```

`CredentialLease` 包含短 TTL、username/password 或 bearer 等 typed material，Debug/Display
始终 redacted，Drop 尽力清零内存。不得 serde、clone 到持久对象或进入 event/error。

exchange 请求绑定 tenant、MediaBinding、operation step、media node instance 和 purpose。
handle 不可跨 tenant/resource/purpose 使用。lease 到期后按协议需要重新 exchange，不持久化
解析后的 secret。

## 2. CRED-02：SecretExchange client

由 gRPC adapter 实现并通过 SDK service slot 注入 Proxy/Snapshot module：

- 只连接配置的 signaling endpoint；
- 强制 mTLS 与 accepted contract version；
- 有 bounded concurrency/deadline；
- 不自动重试 UNKNOWN；
- cache 仅限 lease TTL 内、按 tenant+handle+purpose；
- rotation/revocation 立即失效；
- metrics 只记录 result/reason，不记录 handle/tenant 动态 label。

feature-off 或 provider 未注册时，带 credential handle 的操作返回 Unavailable/NOT_APPLIED。

## 3. CRED-03：URL 迁移

- Proxy 请求出现 `user:pass@host` 直接 InvalidArgument。
- `ProxyInfo.source`、query、event、audit 只保存 `scheme://host[:port]/safe-path` 的脱敏表示。
- URL query 中可能含 token 的 key 按 denylist 移除；未知 query 默认不写日志。
- 旧内联用户名/密码配置在迁移窗口给出启动错误或明确 deprecated，不静默转换。
- RTSP driver 只在发 Authorization 时接触临时 credential material。

## 4. FETCH-01：Snapshot Fetch API

扩展 `SnapshotApi`：

```text
fetch_snapshot(ctx, FetchSnapshotRequest) -> SnapshotHandle
```

请求字段固定为：

```text
source_url
credential_handle?
destination policy
expected media type/format
timeout
max bytes/dimensions
```

不接受服务器文件路径、任意 storage directory、shell command 或 client 自定义 header。
destination 是配置命名 policy/FileStore namespace。

## 5. FETCH-02：Outbound URL policy

- Snapshot Fetch 只允许 HTTP/HTTPS。
- Pull Proxy 只允许 RTSP/RTSPS（其他已交付 proxy operation 另列 capability）。
- scheme/port 有 allowlist；
- 禁止 loopback/private/link-local/multicast/unspecified，除非部署 CIDR 显式允许；
- DNS 的所有 A/AAAA 均验证；
- 实际 connect 固定到已验证 IP，Host/SNI 保留原 host；
- 每次 redirect 重新执行 scheme/port/DNS/CIDR 验证；
- 限制 redirect 次数、URL 长度、DNS answers、connect/read total deadline；
- HTTPS/RTSPS 验证证书，不提供全局 insecure 开关。

把现有 Proxy SSRF/DNS pinning 抽象为 SDK policy port 与生产 provider，Proxy/Snapshot 通过
EngineContext 注入复用；不得在新 module 再复制一套特殊分支。

## 6. FETCH-03：内容与存储

- 只接受配置允许的 image content type，并以实际 magic/decoder 再验证；
- response bytes、dimensions、decode pixel count 有上限；
- streaming 写入有界 buffer/临时受控文件；
- 成功后通过 `MediaFileStoreApi` 原子提交；
- 失败/取消删除临时文件和 credential lease；
- JPEG 为本期生产输出；未支持格式返回 Unsupported；
- response 只返回 SnapshotId/FileHandle/metadata，不返回路径。

## 7. Admission 与 outcome

Fetch/Proxy 在 exchange、DNS、connect、file/port 分配前执行 MediaAdmissionApi、fencing 和
capacity。各阶段：

- policy/exchange/DNS 拒绝：NOT_APPLIED；
- 已建立 proxy/file commit：APPLIED + resource ref；
- connect/commit 状态无法确认：UNKNOWN + reconciliation；
- Deny 不留下 idempotency success、worker、port、file 或 lease。

## 8. 验收

- userinfo、恶意 query、非法 scheme/port、redirect downgrade。
- DNS rebinding、混合 public/private answers、IPv4-mapped IPv6。
- TLS identity failure、redirect loop、slow body、oversize、decompression bomb、wrong magic。
- credential wrong tenant/resource/purpose、expiry、rotation、revocation。
- log/audit/error/store 全量扫描无 password/Authorization/userinfo/path。
- fetch 成功返回可独立解码 JPEG；cancel/deny/disk failure 无临时文件。
