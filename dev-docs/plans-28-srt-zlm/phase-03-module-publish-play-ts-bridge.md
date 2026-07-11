# Phase 03 — Module 推/拉业务、TS-only 与引擎桥接

- **状态**: 待执行
- **范围**: 在 Phase 01 语义与 Phase 02 传输观测就绪后，对齐 ZLM `SrtTransportImp` 的 publish/play 业务路径：握手后分流、鉴权、租约、TS demux/mux、无源/冲突处理、module 源文件拆分。
- **完成标准**: 无 `m` 默认拉流可用；`m=publish` 推 TS 入 engine；拉流出 TS；非法 streamid/鉴权失败/无源/租约冲突行为明确；`module.rs` 拆分到可维护体量；跨协议抽检通过。

---

## 实现概览

对齐 ZLM 分支：

```text
onHandShakeFinished(streamid):
  parseStreamid
  if m == publish:
    emitOnPublish → TS decoder → muxer/tracks
  else:
    emitOnPlay → find TS source → ring attach → send TS
```

Cheetah 映射：

```text
SrtDriverEvent::Connected { stream_id }:
  classify + auth (Phase 01)
  if Publish:
    acquire_publisher → MpegTsDemuxer → PublisherSink
  if Request|Play:
    subscribe → MpegTsMuxer → SendPayload
```

参考：

| 来源 | 路径 |
|------|------|
| ZLM 业务 | `SrtTransportImp.cpp` |
| ZLM TS | `src/TS/*`、`src/Rtp/TSDecoder.*` |
| 本地 module | `crates/protocols/srt/module/src/module.rs` |
| codec | `cheetah-codec` `MpegTsDemuxer` / `MpegTsMuxer` |
| 基线设计 | [plans-28-srt/srt-design.md](../plans-28-srt/srt-design.md) |

---

## 3.1 连接后分流（ZLM 兼容）

**输入**: `Connected { peer_id, remote, stream_id }`

**步骤**:

1. 无 `stream_id` 且无 job forced mode → 拒绝（或仅当配置了 default_publish_stream_key 且 default_mode=publish 时兼容，**严格 ZLM 模式应拒绝**）。
2. `parse_srt_stream_id` + default_vhost 填充。
3. 计算 mode（Phase 01 默认 request）。
4. `authorize_stream` / webhook hook。
5. 映射 `StreamKey`。
6. 分支 publish / play。

失败一律：`driver.Close` + metrics `handshake_reject` / `auth_reject` + 日志。

---

## 3.2 Publish 路径

对齐 ZLM `emitOnPublish` + `onSRTData`：

| 步骤 | 行为 |
|------|------|
| 租约 | `acquire_publisher`；冲突 → 断开并 Conflict 指标 |
| demux | 每 `Payload` 喂入 `MpegTsDemuxer` |
| track | `TrackInfo` Ready 后更新 |
| frame | 时间戳归一化后 `PublisherSink` |
| 断连 | flush demux；释放租约 |
| 非 TS | 配置仅 mpegts；运行期 demux 持续失败 → 诊断并可选断开 |

**单发布者独占**：禁止 module 侧多 publisher 并写同一 `StreamKey`。

---

## 3.3 Play / Request 路径

对齐 ZLM `emitOnPlay` + `doPlay`：

| 步骤 | 行为 |
|------|------|
| 查源 | subscribe engine `StreamKey` |
| 等待 | `play_wait_source_timeout_ms`；超时断开（对齐 ZLM 未找到流 shutdown） |
| bootstrap | GOP / tracks Ready 策略保持现有 egress 配置 |
| mux | `MpegTsMuxer` 输出 TS bytes |
| 发送 | `SendPayload`；尊重 send_queue 背压 |
| 慢订阅者 | 不拖死其他连接；DropUntilNextKeyframe 等策略 |

与 ZLM 差异（文档化即可）：ZLM 直接挂 `TSMediaSource` ring；Cheetah 经 engine 统一帧再 mux，利于跨协议。

---

## 3.4 TS-only 硬约束

| 点 | 要求 |
|----|------|
| 配置 | `payload.kind` 仅允许 `mpegts`（解析失败） |
| 推流 | 仅处理 TS 负载；不引入 FLV/PS over SRT |
| 拉流 | 仅输出 TS |
| 错误信息 | 明确 `payload must be mpegts` |

---

## 3.5 Jobs 与 Listener 并存

保持 ingress/egress/relay jobs：

- job 的 `forced_modes` 优先于 streamid 中的 `m`（已有模式）。
- job URL 中的 streamid 使用同一解析器（Phase 01）。
- 重试退避保持；断线后指数退避。

校验：listener 上 OBS 推流与同时存在的 egress job 不互相干扰。

---

## 3.6 Module 拆分（工程约束）

`module.rs` ~1400 行，按 AGENTS 拆分目标：

```text
stream_classify.rs  # classify_stream, stream_key mapping
auth.rs            # authorize + AuthContext
ingress_session.rs # demux + publish
egress_session.rs  # subscribe + mux
jobs.rs            # build_job_plan, reconnect
module.rs          # Module trait, start loop, wiring
```

要求：

- 不改变对外 manifest / config schema 名称（除非 Phase 01 扩展字段）。
- 拆分后 `cargo test -p cheetah-srt-module` 全绿。

---

## 3.7 跨协议抽检

至少保留/重跑：

| 路径 | 期望 |
|------|------|
| SRT publish → RTMP/HTTP-FLV/HLS 播 | 有画面/分片 |
| RTMP publish → SRT play | ffplay 可读 TS |
| SRT → WebRTC WHEP | 视频可用（音频转码与否按现能力） |

不在本阶段新开转码项目。

---

## 3.8 测试清单

| 用例 | 类型 |
|------|------|
| `m=publish` 推流建立租约 | 集成 |
| 无 `m` 默认拉流 | 集成 |
| 拉流无源超时断开 | 集成 |
| 重复 publish 同 key Conflict | 集成 |
| 鉴权失败断开 | 集成 |
| 非法 streamid 断开 | 集成 |
| TS demux 错误诊断 | 单元/集成 |
| jobs forced mode 优先 | 单元 |
| 拆分后公共 API 稳定 | 编译 + 测试 |

建议 fixture streamid：

```text
#!::h=zlmediakit.com,r=live/test,m=publish
#!::r=live/test
#!::r=live/test,m=request,token=secret
```

---

## 3.9 验收命令

```bash
cargo fmt
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-module

# 手工 E2E
ffmpeg -re -stream_loop -1 -i test.ts -c copy -f mpegts \
  "srt://127.0.0.1:9000?streamid=#!::r=live/test,m=publish"
ffplay -i "srt://127.0.0.1:9000?streamid=#!::r=live/test"
```

---

## 关键文件

| 动作 | 路径 |
|------|------|
| 拆/改 | `crates/protocols/srt/module/src/module.rs` 及新文件 |
| 改 | `crates/protocols/srt/module/src/config.rs` |
| 增 | module 集成测试 |
| 参考 | `vendor-ref/ZLMediaKit/srt/SrtTransportImp.cpp` |
| 参考 | `crates/foundation/cheetah-codec` TS API |

---

## 本阶段不做

- FEC（Phase 04）。
- 完整 OBS/VLC 矩阵归档（Phase 05）。
- TS packet 级 SRT→SRT 直通（非目标）。
- 多 vhost 控制面完整产品化（仅 stream key 策略）。
