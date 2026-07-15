# 05 · RTP 与 GB28181 媒体闭环

## 1. 会话模型

`RtpSession` 增加 serde-default 的 `generation: u64`、`updated_at: i64`、`last_error: Option<MediaOperationError>`。创建 generation=1；状态或可变参数实际变化后加一。`UpdateRtpRequest` 必须携带 `expected_generation`，patch 支持 `ssrc`、`payload_type`、`pause_check`，空 patch 为 InvalidArgument。

SSRC/PT 更新不是 registry 字段修改：必须向 protocol-core 发送显式 `UpdateSession` input，core 原子更新 session 和 SSRC 索引，输出 acknowledgement；driver 等待 acknowledgement，orchestrator 成功后再更新公共快照。冲突、重复 SSRC 或 driver 失败时两侧保持旧值。

core 不读取时间；超时检查由 driver 注入 tick。`pause_check=true` 仅暂停超时判定，不停止 socket、解包或媒体发布。

## 2. 传输矩阵

| 模式 | 必须验证 |
| --- | --- |
| UDP passive | 动态/指定端口、首包绑定、SSRC 过滤、端口释放 |
| TCP passive | accept、2-byte framing、断连、重连策略 |
| TCP active | connect API、deadline、取消、远端断开 |
| sender | H264/PS/G.711 packetize、sequence/timestamp、停止 |
| talk | 接收会话关联、音频 payload、独立 backpressure |

所有 queue 有界，慢消费者不能阻塞接收热路径。停止、超时、module restart 都必须取消任务、关闭 socket、释放端口租约、撤销 publisher/subscriber 并发布一次终态事件。

## 3. GB28181 生产流程

测试不实现 SIP，只模拟信令服务器已协商出的媒体参数：

1. 通过 Rust SDK 或 native HTTP 创建 receiver，取得真实端口。
2. 发送有效 RTP/PS，PS 内含可解析视频和音频 access unit。
3. 等待同一 `MediaKey` online，断言 `TrackInfo` 和至少一个 `AVFrame`。
4. 请求关键帧、URL、录制；校验录制文件非空且可解复用。
5. 创建 sender，真实 UDP 对端收到并解析 RTP；talk 路径收到 G.711。
6. 执行 SSRC/PT 更新，旧参数停止匹配，新参数继续出帧。
7. 验证 inactivity timeout、显式 DELETE 和 module restart 后端口可重用。

## 4. 任务与 DoD

- `RTP-01`：core 更新命令、索引原子性及纯单元/属性测试。
- `RTP-02`：driver ack、TCP active/passive、取消与 backpressure 测试。
- `RTP-03`：orchestrator generation、生命周期和事件测试。
- `RTP-04`：native/兼容 adapter 映射。
- `RTP-05`：GB28181 两层生产合同。

DoD 是包可解析、帧进入 engine、文件可读、网络对端收到数据和所有资源释放；仅断言 session state 不合格。

