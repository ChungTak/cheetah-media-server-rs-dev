# 14 · 执行路线与 Agent 交接

## 1. 顺序与依赖

```text
REL-01
  -> CAP-01 -> CAP-02 -> CAP-03/CAP-04
  -> SEC-01/SEC-02/SEC-03
  -> HTTP-01
  -> RTP-01 -> RTP-02 -> RTP-03 -> RTP-04 -> RTP-05
  -> IMG-01 -> IMG-02 -> IMG-03 -> IMG-04
  -> VOD-01 -> VOD-02 -> VOD-03 -> VOD-04
  -> PRX-03 -> PRX-01/PRX-02/PRX-04 -> PRX-05
  -> EVT-01 -> EVT-02/EVT-03 -> EVT-04/EVT-05
  -> HTTP-02/HTTP-03 -> ZLM-01..04 -> SIG-01..06 -> REL-02..04
```

同一时刻只能有一个变更修改 `cheetah-media-api` 公共 traits。RTP core/driver/module 可在接口合入后并行；快照、VOD、proxy 可在 P0 公共契约稳定后并行。

## 2. P0：先停止虚假承诺

| Task | 实施内容 | 完成证据 |
| --- | --- | --- |
| REL-01 | CI 提供精确工具链并恢复默认 cargo | S0 日志 |
| CAP-01 | report、descriptor reason/operations、generation | registry 生命周期测试 |
| CAP-02 | output registry 与 module 注册/注销 | module restart 测试 |
| CAP-03 | registry 驱动 URL | 不活跃 schema 负测 |
| CAP-04 | HMAC URL 签名 | 篡改/过期/轮换测试 |
| SEC-01 | Principal resource grants 和 list 过滤 | 跨租户测试 |
| SEC-02 | deadline helper 贯穿 provider/driver | 无孤儿资源测试 |
| SEC-03 | 指纹幂等 repository | 重放/冲突/并发测试 |
| HTTP-01 | RTP REST 路由和兼容 alias | native HTTP golden/E2E |
| RTP-01..04 | core 更新、driver ack、orchestrator、adapter | 真实 UDP/TCP 矩阵 |
| IMG-01..04 | encode、原子提交、物理删除、adapter | 独立 JPEG 解码/删除检查 |

P0 结束时，暂未完成的 playback/proxy/FFmpeg operation 必须从 Available 列表移除，而不是保留伪实现。

## 3. P1：真实异步能力

| Task | 实施内容 | 完成证据 |
| --- | --- | --- |
| VOD-01..04 | PlaybackApi、MP4 provider、路由、E2E | MP4 帧/seek/scale/EOF |
| PRX-03 | SSRF allowlist 和重绑定防护 | DNS/redirect 负测 |
| PRX-01/02 | RTSP pull、RTMP push 成功流程 | 本地协议对端 + frames |
| PRX-04/05 | 类型化 executor 和 capability | 子进程成功/失败/清理 |
| EVT-01/02 | AdmissionApi 和主路径接入 | deny 无副作用 |
| EVT-03..05 | 管理、translator、队列、真实 HTTP | 事件/准入 E2E |
| SEC-04/05 | mTLS/HMAC 与审计 | 可信边界和脱敏测试 |
| HTTP-02/03 | 新路由与独立客户端 | 完整服务器黑盒 |

## 4. P2：交付证明

`ZLM-01..04` 先重新分类再修高价值接口；`SIG-01` 先建立真实素材，随后完成 `SIG-02..05`，最后 `SIG-06` 统一黑盒 runner。完成 `REL-02..04` 后才填写发布报告。

## 5. 每项任务执行模板

1. 在差距表确认 current state，不改无关代码。
2. 先写失败测试，记录其证明的缺口。
3. 修改最小公共契约；serde 新字段提供向后兼容默认值。
4. 按 Domain → provider/module → adapter 顺序实现。
5. 运行 changed crate 和反向依赖测试；检查取消、失败和重启。
6. 更新本任务文档状态及 [发布证据](15_release_evidence_template.md)，写明真实命令。

交接记录必须包含 task id、已修改公共接口、未完成分支、测试结果、临时 feature/config、可安全回滚点。不得用“基本完成”“应该可用”等不可验证表述。

## 6. 当前执行状态

| 阶段 | 任务 | 状态 | PR |
| --- | --- | --- | --- |
| REL-01 | 精确工具链与默认 cargo | 已完成并合并 | #135 |
| CAP-01..04 | 能力/输出注册表/HMAC 签名 | 已完成并合并 | #136..#139 |
| SEC-01..03 | 授权/deadline/幂等 | 已完成并合并 | #140..#142 |
| HTTP-01 | RTP REST 路由与兼容别名 | 已完成并合并 | #143 |
| RTP-01..04 | 原子更新/driver ack/orchestrator/adapter | 已完成并合并 | #144..#147 |
| IMG-01..04 | JPEG 编码/原子提交/物理删除/兼容下载 | 已完成并合并 | #148..#151 |
| VOD-01..04 | PlaybackApi/MP4 provider/路由/E2E | 已完成并合并 | #152..#155 |
| PRX-01..05 | RTSP pull/RTMP push/DNS/FFmpeg executor/health | 已完成并合并 | #156..#160 |
| EVT-01..05 | Admission/webhook 路由/translator/E2E | 已完成并合并 | #161..#165 |
| SEC-04/05 | mTLS/HMAC TTL/webhook 响应大小限制 | 已完成并合并 | #166/#167 |
| ZLM-01..04 | 接口目录/高价值接口/golden/L3 黑盒 | 已完成并合并 | #168..#171 |
| REL-02/04 | S0-S5 CI 分组、release build、制品归档 | 已完成并合并 | #172 |
| REL-03 | 并发取消、module restart、资源泄漏观测 | 已完成并合并 | #174 |
| SIG-06 | native HTTP 黑盒 runner（smoke） | 已完成并合并 | #175 |
| SIG-01..05 | 公共 fixture 与四类信令 B 层合同 | 已完成并合并 | #176..#179 |
| 14/15 | 执行路线与发布证据 | 本文档更新中 | 本 PR |

### 已验证命令

P0/P1/P2 本地验证命令：

```text
cargo fmt --check
cargo clippy -p cheetah-media-module -- -D warnings
cargo test -p cheetah-sdk --test signal_contracts
cargo test -p cheetah-media-module
cargo build --release -p cheetah-server --features media-control-full
cargo test -p cheetah-server --features proxy-rtsp,snapshot,record --test signal_blackbox -- --test-threads=1
cargo test -p cheetah-server --features rtp,record,snapshot --test signal_blackbox -- --test-threads=1
cargo test -p cheetah-engine --test resource_leak -- --test-threads=1
```

上述命令已在当前 `main` 通过。SIG 任务统一使用 `apps/cheetah-server/tests/signal_blackbox.rs` 中的 B 层黑盒 runner。

### 当前结论

1. `903_api_completion_plan` 所有代码任务已全部完成并合并到 `main`。
2. 四类信令 A/B 层合同（GB28181、ONVIF、HomeKit、Matter）与 native HTTP 黑盒 runner 均已通过 CI（S0–S5）与 Devin Review。
3. 残留工作：合并本 14/15 文档 PR，更新 `release_evidence_v0.1.0.md` 为最终发布证据，等待发布最终签署。

## 7. 最终 DoD

所有任务有唯一 owner/提交/证据；P0/P1/P2 门禁全绿；能力报告只含通过对应等级的 operation；无 release blocker；四类 A/B 合同与完整服务器 smoke 在同一候选制品上通过。

