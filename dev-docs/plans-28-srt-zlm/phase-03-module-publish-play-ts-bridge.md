# Phase 03 — 推/拉业务、TS-only、Module 拆分（可执行）

- **状态**: 待执行  
- **依赖**: Phase 01（classify/auth/streamid）  
- **兼容规范**: [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md) §3、§7  

## 完成标准（DoD）

- [ ] Connected 后状态机对齐参考 §3  
- [ ] 无 `m` 默认拉流 E2E 或集成测试锁定  
- [ ] publish 租约冲突 / 拉流无源超时 / 非法 streamid → Close + 稳定 reason  
- [ ] payload.kind 非 mpegts 配置失败  
- [ ] `module.rs` 拆分后单文件不再远超 800 行  
- [ ] `cargo test -p cheetah-srt-module` 通过  

---

## 任务 3.1 — 固化握手后状态机

### 文件

- `module/src/module.rs` 的 `handle_driver_event`  
- 目标迁入 `ingress_session.rs` / `egress_session.rs`  

### 伪代码（必须实现）

```text
on Connected(peer, stream_id):
  if forced_mode for peer:
      use forced
  else:
      classified = classify_stream(config, stream_id)?
  match classified.mode:
    Publish:
      acquire_publisher(key)
      if err → Close("reject:publish_conflict" or err)
      else insert IngressSession { demuxer, lease, publisher, auth meta }
    Request | Play:
      spawn run_play_session(...)
  on classify/auth err:
      Close("reject:invalid_stream_id" | "reject:auth_rejected")

on Payload(peer, bytes):
  if let Some(ingress) = sessions.get(peer):
      demux + publish frames
  else:
      // 拉流连接忽略媒体，可 debug 日志

on Disconnected:
  release lease; metrics; job retry
```

对照参考 §3.1：鉴权前帧缓存可选；现有实现鉴权在 Connected 同步完成，可保持「先鉴权再收流」。

---

## 任务 3.2 — Publish 路径检查清单

现有 `SrtIngressSession` + `handle_ingress_payload`：

1. 打开 `module.rs` 搜索 `MpegTsDemuxer` / `handle_ingress_payload`。  
2. 确认：  
   - Track Ready 后 `update_tracks` / 等价  
   - frame 经 `PublisherSink`  
   - 断开 `release_publisher`  
3. 补充：demux 持续错误计数超过阈值可 Close（可选，建议做）。  
4. 测试：同 key 第二次 publish → 第二连接被关。  

---

## 任务 3.3 — Play 路径检查清单

现有 `run_play_session`：

1. 搜索函数，确认：  
   - `play_wait_source_timeout_ms`  
   - `MpegTsMuxer`  
   - `SendPayload`  
   - cancel / 源结束关闭  
2. 对齐参考 §3.2：超时/无源必须 Close，reason=`reject:stream_not_found` 或 `reject:play_timeout`。  
3. 测试：无源 play → 超时断开（可用短 timeout 配置）。  

---

## 任务 3.4 — TS-only 硬校验

### 文件

- `module/src/config.rs` `from_value` 或 validator  

```rust
// payload.kind 必须 "mpegts"（大小写不敏感可）
// 否则 InvalidArgument
```

模块 schema validator 已存在（`SrtModuleConfig::from_value`），在 `from_value` 后增加 `validate(&self) -> Result<(), String>`。

---

## 任务 3.5 — Module 拆分（强制）

### 目标结构

```text
crates/protocols/srt/module/src/
  lib.rs                 # mod 声明
  config.rs
  metrics.rs
  http.rs
  module.rs              # ModuleFactory + Module impl + start 循环
  stream_classify.rs     # Phase 01
  auth.rs                # Phase 01
  ingress_session.rs     # IngressSession + payload handler
  egress_session.rs      # run_play_session
  jobs.rs                # build_job_plan + retry_delay + schedule
```

### 步骤

1. 创建空文件 + `mod` 声明。  
2. 按符号移动，保持 `pub(crate)`。  
3. 每移一块就 `cargo test -p cheetah-srt-module`。  
4. 禁止一次巨型 rename 后无法编译。  

### 行数目标

- `module.rs` 最终以 trait/lifecycle/wiring 为主，建议 <600 行。  

---

## 任务 3.6 — 集成测试用例表

在 `module/src/` 内测或 `module/tests/`：

| 测试 | 步骤 | 期望 |
|------|------|------|
| `default_no_m_is_play` | classify `#!::r=live/test` | mode Request/Play，非 Publish |
| `m_publish_is_publish` | `...,m=publish` | Publish |
| `auth_params_include_m` | classify | auth_params 含 m |
| `strict_r_one_segment_fails` | `#!::r=live` | Err |
| `payload_kind_rejects_non_ts` | config kind=flv | from_value/validate Err |

若可启动 driver（重测成本高），优先单测 classify；E2E 放 Phase 05。

Jobs：forced mode 仍优先于 streamid（现有逻辑，加单测锁定）。

---

## 任务 3.7 — 跨协议抽检（手工或 ignore）

```bash
# SRT publish → 其它协议播：依赖 server 已开 rtmp/hls
ffmpeg ... srt://127.0.0.1:9000?streamid=#!::r=live/test,m=publish
# ffplay rtmp://127.0.0.1/live/test

# 其它协议 publish → SRT play
ffplay -i "srt://127.0.0.1:9000?streamid=#!::r=live/test"
```

记录结果到 Phase 05 矩阵即可。

---

## 验收命令

```bash
cargo fmt
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-module
# 确认拆分
wc -l crates/protocols/srt/module/src/*.rs
```

## 本阶段不做

- FEC  
- 完整 OBS/VLC GUI 矩阵（Phase 05）  
- TS 包级 SRT→SRT 直通  
