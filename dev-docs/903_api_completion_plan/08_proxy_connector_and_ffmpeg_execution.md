# 08 · 代理连接器与 FFmpeg 执行

## 1. 代理状态机

统一状态为 `Created -> Connecting -> Connected -> Stopping -> Stopped`，任意运行态可转 `Failed`。create 返回 Created/Connecting，不等待完整媒体；Connected 只能在连接建立且首个有效 track/frame 已进入或离开 engine 后设置。`last_error` 使用稳定错误码并清除敏感 URL 信息。

RTSP pull 和 RTMP push 必须使用已有协议 module/driver 路径；proxy module 负责编排、租约、重试、状态和取消，不复制协议状态机。重试采用有上限指数退避并受总 deadline/config 限制。

## 2. SSRF

默认拒绝 loopback、link-local、private、multicast 和未指定地址。配置用 CIDR allowlist 显式开放设备网段；测试环境只开放测试使用的 loopback 段。解析 hostname 后校验全部 A/AAAA，连接时使用已校验地址并在重解析后再次校验，防止 DNS 重绑定。重定向目标重新执行同样策略。

## 3. 类型化 FFmpeg API

删除公共 `command: String` 语义，改为 `FfmpegJobSpec`：profile_id、typed input、typed output、codec/filter options、timeout、资源限制。profile 来自受控配置，调用方不能指定 executable 或任意参数。

异步 API 为 `submit/get/list/wait/cancel`，返回含 id、state、pid（仅内部可见）、timestamps、exit summary 的 handle/status。具体 executor 在 system/runtime 实现：不用 shell，参数逐项传递；限制 stderr 环形缓冲、运行时长、并发数和可选 CPU/内存；cancel 必须 terminate、超时 kill、wait/reap。进程启动失败不得进入 Running。

FFmpeg proxy 由 ProxyApi 编排该 executor，并把任务状态映射到 ProxyInfo。executor/provider 缺失时 capability operations 不列 FFmpeg。

## 4. 任务与验收

- `PRX-01`：真实 RTSP pull 成功链路、状态与清理。
- `PRX-02`：真实 RTMP push 成功链路、状态与清理。
- `PRX-03`：SSRF allowlist、DNS/redirect 二次校验。
- `PRX-04`：类型化 FFmpeg API 和实际进程 executor。
- `PRX-05`：proxy capability 与健康状态接线。

测试启动本地协议服务器，验证 Connected 后确有 frame；删除后连接、任务、租约均消失。FFmpeg 使用受控测试 profile，覆盖成功、非零退出、超时、取消、stderr 截断和不存在 executable。
