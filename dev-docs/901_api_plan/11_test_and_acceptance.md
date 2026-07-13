# 11. 测试与验收

## 1. Domain 单元测试

- MediaKey 构造、默认 vhost、schema 和 StreamKey 转换可逆。
- 分页、cursor、时间单位和过滤条件边界。
- 错误码稳定，unsupported 与 unavailable 可区分。
- 幂等 key 重复 start/stop 的结果一致。
- RecordPlaybackCommand 对 scale/seek 缺 value 时拒绝。
- RTP port/SSRC/TCP mode/reuse-port 组合校验。
- 事件序列、event id 去重和未知枚举兼容。

## 2. Provider/Engine 集成测试

- publisher → engine → subscriber 的 AVFrame/TrackInfo 传播。
- 同一 MediaKey 的多 schema 输出共享状态。
- publisher 独占 lease；第二发布者得到 Conflict。
- provider 重启遵守 `create -> init -> start` 和 `ModuleRestartRequired`。
- record provider 任务状态、文件索引、删除和 playback control。
- RTP provider open/connect/send/timeout/stop 全生命周期。
- 慢事件订阅者不会阻塞媒体发布或其他订阅者。

## 3. Native HTTP 测试

测试每条 native route 的：

- 成功、参数错误、未认证、无权限、找不到、冲突、超时、unsupported。
- JSON schema、分页、RFC3339/毫秒时间单位、request id 和 idempotency。
- 文件下载不能越权读取绝对路径。
- 两个 adapter 同时启用/单独启用时相互独立。

## 4. ZLM 兼容测试

建立不依赖 vendor 源码的 golden fixtures，覆盖：

- `/index/api` 所有目录项的请求参数别名和返回 `code/msg`。
- legacy HTTP 200 语义、`-100/-200/-300/-400/-500/-501` 映射。
- `/index/hook` 的请求字段、响应字段、超时和拒绝策略。
- `startRecord`、`getMP4RecordFile`、`getSnap`、`openRtpServer`、`startSendRtp`、`getMediaList` 等高价值流程。
- stream_id/stream、0/1 与 true/false、秒/毫秒等兼容边界。
- secret 不出现在日志、事件、错误 details 和文件 URL。

## 5. 外部信令 contract test

使用 fake GB28181/ONVIF/HomeKit/Matter client 验证：

- 外部项目能创建 RTP receiver、等待 online、获取播放 URL、请求关键帧并关闭会话。
- 外部项目能开始录制、订阅 record completed、查询文件和控制回放。
- 外部项目能取快照并通过 handle 下载。
- 未实现的信令能力不会被误认为 Cheetah 已实现。

## 6. 性能与安全验收

- 查询和文件列表必须分页且有最大 page size。
- 事件、webhook、RTP 和 proxy 的队列/重试/缓存有上界。
- 热路径无阻塞 I/O；adapter 超时不拖住 engine。
- secret、cookie、密码、SRTP/HAP/Matter 凭据不进入 domain event 和普通日志。
- webhook SSRF、文件路径穿越、任意 FFmpeg 命令、未授权踢流和删文件均有拒绝测试。

## 7. 提交前检查

文档实现对应的 Rust 变更至少执行：

```text
cargo fmt
cargo clippy -p <changed-crate>
cargo test -p <changed-crate>
```

若修改 `cheetah-media-api`、`cheetah-sdk`、`cheetah-engine`、`cheetah-codec` 或协议公共层，继续运行受影响 workspace/module 测试。不要例行使用 `--all-features`。

## 8. 文档验收

- 计划目录存在 README 和 01-11 全部章节。
- 新文档不依赖外部源码目录、外部链接或执行体无法访问的路径。
- 每个 API 目录项都有 domain 映射、错误行为、未实现策略和测试归属。
- 发生 crate、分层、公共 API 或媒体模型变化时同步 `SystemArchitecture.md`、`AGENTS.md` 及相关 README。

