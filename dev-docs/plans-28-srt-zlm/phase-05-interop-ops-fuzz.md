# Phase 05 — 互操作、Fuzz 与运维收口

- **状态**: 待执行
- **范围**: 将 ZLM 文档中的 OBS / FFmpeg / ffplay / VLC 用法固化为可重复验收矩阵；补齐 fuzz/corpus 与运维指标/文档；收口 Phase 01–04 的遗留测试。
- **完成标准**: 互操作矩阵有命令、期望与结果记录模板；核心 fuzz target 可短跑 smoke；`/srt/metrics` 字段表完整；运维文档可独立按图操作。

---

## 实现概览

参考：

| 来源 | 路径 |
|------|------|
| ZLM 用法 | `vendor-ref/ZLMediaKit/srt/srt.md` |
| 基线 ops | [plans-28-srt/srt-ops-interop.md](../plans-28-srt/srt-ops-interop.md) |
| 本地 fuzz | `crates/protocols/srt/fuzz/` |
| metrics | `crates/protocols/srt/module/src/{http,metrics}.rs` |

本阶段以 **验证与文档** 为主，代码改动限于测试 harness、metrics 补全、小修复。

---

## 5.1 外部互操作矩阵

### 推流

| ID | 客户端 | 命令 / 配置 | 期望 |
|----|--------|-------------|------|
| P1 | FFmpeg | 见下 | Cheetah 出现 `live/test` 流；tracks Ready |
| P2 | OBS | `srt://HOST:9000?streamid=#!::r=live/test,m=publish` | 同上 |
| P3 | srt-live-transmit | MPEG-TS → SRT caller publish streamid | 同上 |
| P4 | FFmpeg + passphrase | 加密推流 | 成功；错误口令失败 |

```bash
ffmpeg -re -stream_loop -1 -i test.ts -c:v copy -c:a copy -f mpegts \
  "srt://127.0.0.1:9000?streamid=#!::r=live/test,m=publish"
```

### 拉流

| ID | 客户端 | 命令 / 配置 | 期望 |
|----|--------|-------------|------|
| L1 | ffplay | 见下 | 可播 |
| L2 | VLC | URL `srt://HOST:9000`；偏好设置 streamid `#!::r=live/test` | 可播 |
| L3 | FFmpeg 录制 | `ffmpeg -i srt://...?streamid=#!::r=live/test -c copy out.ts` | 文件可读 |
| L4 | 无 m 默认拉流 | 仅 `#!::r=live/test` | 不进 publish |

```bash
ffplay -i "srt://127.0.0.1:9000?streamid=#!::r=live/test"
```

### 负例

| ID | 场景 | 期望 |
|----|------|------|
| N1 | 无 m 且 default=request 时误当推流 | 不产生 publish 租约 |
| N2 | 重复 publish 同 key | 第二路拒绝 |
| N3 | 错误 token | 断开 + auth_reject |
| N4 | 非法 streamid | 断开 |
| N5 | peer version 过低（若可构造） | 断开 + version reject |
| N6 | FEC required 但对端无 FEC | 断开（Phase 04） |

### 跨协议抽检

| ID | 路径 | 期望 |
|----|------|------|
| X1 | SRT → RTMP/HLS | 播或分片 OK |
| X2 | RTMP → SRT play | ffplay OK |
| X3 | SRT → WHEP | 视频 OK（音频按能力） |

### 弱网（承接 Phase 02）

复跑 1%/5%/12% loss 与限速；记录 retransmit/fec 指标截图或 JSON。

### 结果记录模板

在 `dev-docs/plans-28-srt-zlm/` 或复用 `plans-28-srt/srt-ops-interop.md` 追加：

```text
日期 | 用例ID | 环境 | 结果 | 指标摘要 | 备注
```

建议 module 测试中用 `#[ignore]` + 环境变量门控（如 `CHEETAH_SRT_INTEROP=1`），避免 CI 无 GUI/无工具失败。

---

## 5.2 VLC / OBS 专项说明（写入 ops）

### OBS

- 服务：自定义；服务器填 `srt://IP:9000?streamid=#!::r=live/test,m=publish`
- 注意 URL 编码与 `#` 在部分版本中的处理；失败时改用 streamid 独立配置项（若 OBS 版本支持）。

### VLC

- 偏好设置 → 输入/编解码器 或 串流输出 → 访问输出 → **SRT** → streamid = `#!::r=live/test`
- 打开网络串流：仅 `srt://IP:9000`
- 验证点：HS 扩展携带 streamid，module 日志可见 `Connected { stream_id: Some(...) }`

---

## 5.3 Fuzz 与 Property

现有：

- `fuzz_srt_url`
- `fuzz_stream_id`
- `fuzz_driver_packet`

增量建议：

| Target | 输入 | 期望 |
|--------|------|------|
| stream_id 增强 | 随机 `#!::` 字段序 / 超长 | 不 panic |
| version parse | 随机版本串 | 不 panic |
| fec layout | 随机 cols/rows | 校验失败安全 |
| handshake 脏包 | 随机 UDP payload | driver 不崩（已有 packet fuzz 加强 corpus） |

执行：

```bash
# 短 smoke（示例）
cd crates/protocols/srt/fuzz
cargo fuzz run fuzz_stream_id -- -runs=1000
cargo fuzz run fuzz_srt_url -- -runs=1000
cargo fuzz run fuzz_driver_packet -- -runs=1000
```

Property tests：

- streamid 字段排列交换等价
- auth_params 键集合稳定性
- version 比较传递性

---

## 5.4 运维与指标收口

### HTTP

| 路径 | 内容 |
|------|------|
| `GET /srt/metrics` | Prometheus |
| `GET /srt/metrics.json` | JSON |

### 指标清单（应文档化）

```text
srt_connections / srt_publish_connections / srt_play_connections
srt_bytes_in_total / srt_bytes_out_total
srt_packets_in_total / srt_packets_out_total
srt_retransmit_total
srt_receiver_lost_total / srt_receiver_duplicate_total
srt_rtt_micros / srt_jitter_micros（或 histogram）
srt_send_queue_full_total
srt_key_refresh_total
srt_disconnect_total{reason}
srt_handshake_reject_total{reason}
srt_auth_reject_total
srt_fec_*（Phase 04）
```

### 日志字段

`peer_id, remote, stream_id, vhost, app, stream, mode, reject_reason`

### 健康

- 模块 Running + listener 已绑定可作为健康信号。
- 若仅有全局 engine health，在 ops 中说明 scrape 建议。

### 文档产物

更新或新增：

- 本目录简短 `README` 可省略（index 已是入口）
- 在 [plans-28-srt/srt-ops-interop.md](../plans-28-srt/srt-ops-interop.md) 增加「ZLM 兼容语义」一节链接到本计划
- 或在本计划 Phase 05 完成后于 index 状态栏标注互操作结果摘要

---

## 5.5 回归总命令

```bash
cargo fmt
cargo clippy -p cheetah-srt-core
cargo clippy -p cheetah-srt-driver-tokio
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-core
cargo test -p cheetah-srt-driver-tokio
cargo test -p cheetah-srt-module
cargo test -p cheetah-srt-property-tests
```

手工：

```bash
# 推
ffmpeg -re -stream_loop -1 -i test.ts -c copy -f mpegts \
  "srt://127.0.0.1:9000?streamid=#!::r=live/test,m=publish"

# 拉
ffplay -i "srt://127.0.0.1:9000?streamid=#!::r=live/test"

# 指标
curl -sS http://127.0.0.1:8080/srt/metrics   # 端口以实际 control 为准
```

---

## 5.6 完成定义（整计划）

当且仅当：

1. Phase 01–04 各自完成标准满足（FEC 若延期须在 index 标明剩余范围）。
2. 互操作矩阵 P1/L1 必过；P2/L2 至少手工记录一次。
3. 默认无 `m` 为拉流的行为有测试锁定。
4. fuzz smoke 可运行。
5. metrics 字段与 ops 命令文档齐全。

---

## 关键文件

| 动作 | 路径 |
|------|------|
| 增 | module/driver `#[ignore]` 互操作测试 |
| 改 | `fuzz/` targets / corpus |
| 改 | `metrics.rs` / `http.rs`（补字段） |
| 改 | ops 文档链接 |
| 参考 | `vendor-ref/ZLMediaKit/srt/srt.md` |

---

## 本阶段不做

- 新协议功能（应回 Phase 01–04）。
- 内置 TURN/转码/UI。
- 强制 CI 依赖 OBS/VLC GUI。
