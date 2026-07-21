# 11 · 测试、互操作、Fuzz 与 CI

## 1. Fixture 规则

- 每个 fixture 记录来源项目/设备、revision/型号、transport、framing、container、PT、codec、SSRC、
  预期输出和脱敏方式。
- 优先保存最小合成/脱敏 packet sequence、pcap 派生摘要或生成器；不提交凭据、个人数据或版权
  不明的大型媒体。
- 每个 compat rule 至少有一个正例、一个关闭 profile 的拒绝例和一个 malformed 邻接例。

## 2. 测试矩阵

| 层 | 必测场景 | Gate |
| --- | --- | --- |
| codec/core unit | split every byte、PSM late/change、PES zero、H264/H265/AAC/G711、seq/ts wrap | TST-CORE |
| property | parser roundtrip、任意 fragmentation、reorder/duplicate、bounded state invariant | TST-PROP |
| fuzz | media API/PS/PES/RTP/RTCP/framing/JTT/Ehome；no panic/OOM/unbounded loop | TST-FUZZ |
| driver | UDP/TCP active/passive、2/4-byte、mux/separate RTCP、half-close、slow peer | TST-DRV |
| module | admission、capacity、port/publisher、idempotency、cancel、restart、rollback | TST-MOD |
| external API | typed mapper、two-stage open/connect、fencing/event/query | TST-EXT |
| interop | ABL/ZLM/SMS style + 已授权真实设备 | TST-IOP |
| operational | loss/reorder/NAT/port exhaustion/registry outage/cert rotation/drain | TST-OPS |

## 3. 关键验收场景

1. admission 返回 Deny，前后 port/socket/task/worker/lease/resource/idempotent-success 计数完全相同。
2. 在 lifecycle 每一步注入失败/取消，rollback 后资源基线一致。
3. 同 idempotency request response loss 后重试，只存在一个有效 RTP session。
4. old owner、old node instance、old generation mutation 返回 fencing error 且无网络副作用。
5. TCP 任意切分/粘包、错误长度和垃圾前缀不会无限增长；恢复预算耗尽后可预测关闭。
6. 乱序、重复、序列/时间戳回绕输出单调媒体时间线，慢 session 不影响其他 session。
7. 无 PSM 与动态 PT 的 H264/H265/AAC/G711 fixture 被正确规范化，歧义输入明确拒绝。
8. JT1078 2013/2019 双向组包/拆包；Ehome2 实包；Ehome5 未 gate 时 Unsupported。
9. 生产制品无 GB SIP listener/parser/task；第三方 two-stage open/connect 超时后资源自动收敛。
10. talk/playback/download 取消或远端失败无端口、reader、blocking worker 和 publisher 泄漏。

## 4. 互操作套件

| Suite | 内容 | 产物 |
| --- | --- | --- |
| IOP-ABL | 2/4-byte、PS/raw、PT96/97/98/99、G711、JTT2013/2019 | packet transcript + decoded sample |
| IOP-ZLM | SSRC fallback、PS/TS sniff、TCP resync、RTCP、active/passive | session/event/stats report |
| IOP-SMS | REST media aliases、SSRC、live/playback/download/talk media binding | black-box report |
| IOP-DEVICE | 至少覆盖 UDP、TCP active、TCP passive、H264、H265、PCMA talk | sanitized pcap + player evidence |

互操作只验证本仓媒体责任；REGISTER/Catalog/RecordInfo/Alarm/SDP 等不属于本项目测试范围。

## 5. CI lanes

- `gb-core`: fmt、clippy、unit/property、runtime boundary、dependency boundary。
- `gb-driver`: Tokio I/O matrix，使用独立 target dir/端口 namespace。
- `gb-module`: feature-off/on、fake admission、lifecycle E2E、REST compatibility adapter。
- `gb-fuzz-smoke`: 固定 corpus 与短时 sanitizer；nightly 执行长 fuzz。
- `gb-interop`: reference fixtures；需要设备的 lane 显式 gated，不以跳过标 PASS。
- `external-media-contract`: 第三方 simulator 与真实 HTTP/gRPC adapter 运行同一套媒体 API 测试。
- `gb-security`: malformed/limits/secret scan/mTLS/source spoof。
- `gb-soak`: nightly/候选版本执行故障注入与 24 小时长稳。

## 6. 最低命令

```bash
cargo fmt --check
cargo clippy -p cheetah-gb28181-core -p cheetah-gb28181-driver-tokio \
  -p cheetah-gb28181-module --tests -- -D warnings
cargo clippy -p cheetah-rtp-core -p cheetah-rtp-driver-tokio \
  -p cheetah-rtp-module --tests -- -D warnings
cargo test -p cheetah-gb28181-core
cargo test -p cheetah-gb28181-driver-tokio
cargo test -p cheetah-gb28181-module
cargo test -p cheetah-gb28181-property-tests
cargo test -p cheetah-rtp-core
cargo test -p cheetah-rtp-driver-tokio
cargo test -p cheetah-rtp-module
cargo test -p cheetah-rtp-property-tests
./dev-scripts/check_runtime_boundaries.sh
```

共享 codec/media API/control plane 改动还必须运行其 crate 及所有反向依赖测试。不要例行使用
`--all-features`；按 capability/profile 组合建立明确 lane。

## 7. 长稳退出条件

候选制品执行 24 小时混合收发、create/stop、loss/reorder、peer reconnect、module restart、controller
短时中断与证书轮换。退出时没有存活的已停止 session、未归还端口/permit/lease、孤立 task/worker；
RSS/queue/resource 曲线无持续无界增长。性能回归与允许阈值由同硬件基线报告逐项签署，不用跨机器
绝对数替代回归判断。
