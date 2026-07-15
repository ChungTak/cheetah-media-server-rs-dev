# 15 · 发布证据模板

> 复制本模板生成候选版本报告。未实际运行的项目保持 `[ ]`，不得根据代码阅读填写通过。

## 1. 候选版本

| 字段 | 值 |
| --- | --- |
| revision | `<commit>` |
| build profile/features | `<profile>` |
| rustc/cargo | `<exact output>` |
| lockfile hash | `<sha256>` |
| platform | `<os/arch>` |
| evidence location | `<artifact id/path>` |

## 2. 任务证据

| Task ID | Owner | Revision | Command | Result | Artifact | Residual risk |
| --- | --- | --- | --- | --- | --- | --- |
| `<ID>` | `<name>` | `<sha>` | `<exact command>` | PASS/FAIL | `<log>` | `<none or issue>` |

每个 CAP、HTTP、RTP、IMG、VOD、PRX、EVT、SEC、ZLM、SIG、REL 任务至少一行。测试结果必须对应当前 revision；缓存的旧 revision 结果无效。

## 3. 能力证据

| Capability.operation | Provider/state | L0 | L1 | L2 | L3/L4 | 已知限制 |
| --- | --- | --- | --- | --- | --- | --- |
| `<cap.op>` | `<provider/state>` | `<test>` | `<test>` | `<test>` | `<test>` | `<text>` |

运行服务器后附 `/api/v1/media/capabilities`、details 和 active output endpoints 的脱敏快照，并证明三者与实际监听、连接结果一致。

## 4. 兼容与信令矩阵

| Contract | Rust SDK | Native HTTP | Real media validation | Cleanup | Result |
| --- | --- | --- | --- | --- | --- |
| GB28181 media | `<artifact>` | `<artifact>` | PS/RTP parsed | ports/tasks | PASS/FAIL |
| ONVIF media | `<artifact>` | `<artifact>` | RTSP frames/JPEG | proxy/files | PASS/FAIL |
| HomeKit media | `<artifact>` | `<artifact>` | audio/video/RTP | subscriptions | PASS/FAIL |
| Matter media | `<artifact>` | `<artifact>` | files/events | subscription | PASS/FAIL |

另附兼容接口目录及每项最高 L0–L4 等级；低于 L1 的项目标为 wire-only。

## 5. 门禁清单

- [ ] 精确工具链、fmt、clippy、changed-crate tests 通过。
- [ ] 共享层反向依赖和发布 profile 通过。
- [ ] RTP UDP/TCP、更新、超时、端口回收通过。
- [ ] JPEG 独立解码、文件物理删除通过。
- [ ] MP4 playback、RTSP pull、RTMP push、FFmpeg executor 通过。
- [ ] admission、Webhook 投递、资源授权、deadline、幂等通过。
- [ ] native/兼容 HTTP 黑盒通过。
- [ ] 四类信令 A/B 合同通过。
- [ ] 并发取消、module restart、资源泄漏观测通过。
- [ ] [发布阻断项](13_test_toolchain_ci_and_release_gates.md)逐项确认为不存在。

## 6. 失败与豁免

发布阻断项不允许豁免。其他失败必须记录 issue、影响的 operation、临时 capability state、owner、到期版本和用户可见限制；将 capability 降为 Unavailable/Degraded 后仍需重新运行能力真实性测试。

## 7. 签署

| 角色 | 结论 | 姓名/时间 | 证据 |
| --- | --- | --- | --- |
| implementation | APPROVE/REJECT |  |  |
| API compatibility | APPROVE/REJECT |  |  |
| security | APPROVE/REJECT |  |  |
| release | APPROVE/REJECT |  |  |
