# 01 · 审计基线与差距登记

## 1. 判定规则

状态仅使用 `Implemented`、`Partial`、`Stub`、`Missing`。`Implemented` 必须同时具备生产 provider、真实副作用、成功与失败测试；本文件没有任何待修能力可预先标成完成。

## 2. 当前事实

| ID | 能力 | 当前证据 | 状态 | 发布风险 |
| --- | --- | --- | --- | --- |
| CAP-01 | capability registry | 有 set、descriptor 和 provider registry，但 descriptor 未成为运行时事实源 | Partial | 客户端选择不可用能力 |
| CAP-02 | 播放 URL | resolver 固定生成多种 schema，`available` 与 module 状态无关 | Stub | 返回不可连接 URL |
| CAP-03 | URL 签名 | 使用自定义非密码散列 | Stub | 签名可伪造 |
| HTTP-01 | native RTP | 缺 connect/get/update/delete 的完整 REST 暴露 | Partial | 外部信令无法管理会话 |
| RTP-01 | RTP 接收/发送 | module 有真实 UDP、发送、talk 和端口释放测试 | Partial | TCP/更新/合同证据不足 |
| RTP-02 | SSRC/PT 更新 | API 存在，底层返回不支持或未原子应用 | Stub | 设备切换参数失败 |
| IMG-01 | 快照编码 | MJPEG 外的帧被存为 `.bin`，即使请求 JPEG | Stub | 下游获得损坏图片 |
| IMG-02 | 快照目录删除 | 清 registry，未证明删除文件存储和物理文件 | Stub | 隐私与磁盘泄漏 |
| VOD-01 | 录制回放 | record 内只维护暂停、倍率、位置状态 | Stub | 不产生回放媒体 |
| VOD-02 | MP4 VOD | MP4 core/driver/module 已有真实状态机、文件读取和 engine bridge | Implemented | 尚未接入统一 PlaybackApi |
| PRX-01 | RTSP/RTMP proxy | 有 feature-gated connector 路径，测试以登记/拒绝为主 | Partial | 成功链路未知 |
| PRX-02 | FFmpeg | `command: String` 加内存登记，无进程执行和状态 | Stub | 返回虚假任务成功 |
| EVT-01 | typed event bus | 内部事件和 dispatcher 已存在 | Partial | 外部管理与覆盖不足 |
| EVT-02 | 同步准入 | decision client 只在自身测试被调用，发布/播放主路径未接入 | Missing | 鉴权策略不生效 |
| SEC-01 | authorization | Principal 仅全局 scopes，无 vhost/app/stream grant | Partial | 跨租户越权 |
| SEC-02 | deadline | context 可携带 deadline，provider 未统一消费 | Stub | 超时后仍产生副作用 |
| SEC-03 | idempotency | adapter 有基础支持，缺统一指纹冲突和全能力覆盖 | Partial | 重复任务或键误复用 |
| ZLM-01 | 兼容目录 | 路由和 golden 较全，但 L1 被过度标记 | Partial | 兼容承诺失真 |
| SIG-01 | GB28181 | production contract 发包但未断言引擎 track/frame | Partial | 不能证明媒体进入系统 |
| SIG-02 | ONVIF | 只验证 SSRF 拒绝，未成功拉取 RTSP | Stub | 局域网摄像机不可交付 |
| SIG-03 | HomeKit | 绕过正式数据面，缺音频、慢消费者和清理 | Stub | 集成边界错误 |
| SIG-04 | Matter | 建立事件订阅后未验证事件内容 | Stub | 异步闭环缺失 |
| SIG-05 | HTTP 合同 | 无独立进程从 native HTTP 完成四类流程 | Missing | 只能同进程集成 |

## 3. 不得破坏的已实现基础

- `cheetah-media-api` 已有媒体键、请求上下文、控制/录制/快照/代理/RTP traits。
- `MediaServices` 支持 provider 动态注册、替换和 generation。
- RTP module 已有共享 orchestrator，不能另建第二套 session registry。
- MP4 的 Sans-I/O VOD 与 Tokio 文件驱动应复用，禁止在 record module 重写 demux。
- 事件总线和 Webhook translator/sender 应扩展，禁止再造协议专用事件总线。

## 4. 收敛纪律

每个任务完成时必须把本表对应项更新为证据链接和测试命令；`Unsupported` 是允许的稳定结果，但 capability 不得同时宣称 Available。发现新差距时使用现有前缀追加编号，并同步 [执行路线](14_execution_roadmap_and_agent_handoff.md)。

