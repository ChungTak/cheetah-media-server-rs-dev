# 05 · Proxy Provider 与输出 URL

## 1. 目标架构

新增职责独立的 `cheetah-proxy-module`，使用 `cheetah-connector` 和 engine 数据面实现拉流、推流和受控 FFmpeg proxy。`cheetah-engine` 不直接依赖具体协议 module。

```text
pull: source protocol → connector pull → AVFrame/TrackInfo → engine publisher
push: engine subscriber → connector push → destination protocol
ffmpeg: typed job spec → FfmpegApi → monitored process/session
```

## 2. Pull proxy

`create_pull_proxy` 必须：

- 校验 URL scheme、目标 MediaKey 和权限。
- 获取目标 publisher lease；冲突时失败，不覆盖现有 publisher。
- 按 scheme 选择 connector capability；supports 与 open 行为一致。
- 把 track/frame 规范化后写入 engine。
- 维护 Created/Connecting/Running/Retrying/Stopping/Stopped/Failed 状态。
- 使用有限次数或有界指数退避；取消后不继续重连。
- 注册 proxy session、metrics 和 ProxyStateChanged 事件。

ONVIF P0 路径要求 RTSP pull 真实可用。

## 3. Push proxy

`create_push_proxy` 必须打开 engine subscriber，并把 frame 写入 connector push。慢目标不能阻塞其他订阅者；队列满按策略丢弃或断开。关闭顺序为停止接收新帧、flush 有界队列、关闭 connector、注销 session。

首批真实支持按现有 connector capability 决定，至少形成一个 RTMP push 成功路径；RTSP/SRT/WebRTC 根据对应 connector 能力逐项启用，不能提前宣告。

## 4. FFmpeg proxy

`FfmpegProxyRequest` 转换为 typed `FfmpegJobSpec`：input、output、codec policy、尺寸、音视频开关、超时、资源限制。禁止传入 shell 字符串或任意额外参数；二进制路径由部署配置控制。

进程退出、超时、取消、stderr 摘要映射为领域错误和 proxy event。日志必须过滤 URL 中的用户信息和 token。

## 5. URL Resolver

新增 `MediaUrlResolverApi`，协议 module 注册 URL template/capability：

```rust
pub trait MediaUrlResolverApi: Send + Sync {
    async fn resolve_urls(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
        schemas: &[MediaSchema],
    ) -> Result<Vec<MediaUrl>>;
}
```

URL 使用配置的 public host、端口、TLS 和 path；不能从未经信任的 Host header 直接生成。需要 token 的 URL 使用短期签名和过期时间。StreamInfo 和 `getStreamUrl` 共用 resolver。

## 6. 存储和幂等

proxy registry 有固定上限。相同 idempotency key + 相同请求返回原 proxy；不同请求返回 Conflict。delete 对 Running/Retrying 都必须取消实际任务，不能只删 metadata。

## 7. 任务与验收

| ID | 任务 | 验收 |
| --- | --- | --- |
| S4-T1 | proxy module/registry | 上限、幂等、restart |
| S4-T2 | RTSP pull | 真实源→engine 出帧 |
| S4-T3 | RTMP push | engine→真实接收端出帧 |
| S4-T4 | retry/cancel | 有界退避、无孤儿任务 |
| S4-T5 | FFmpeg typed job | 注入攻击拒绝 |
| S4-T6 | URL resolver | 多 schema、TLS、签名过期 |

```bash
cargo test -p cheetah-connector --features full
cargo test -p cheetah-proxy-module
cargo test -p cheetah-media-module proxy
```

