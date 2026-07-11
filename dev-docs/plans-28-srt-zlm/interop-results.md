# SRT ZLM 兼容互操作记录

本文件记录 `cheetah-srt-module` 与外部 SRT 客户端/工具的对齐结果。
自动化测试壳位于 `crates/protocols/srt/module/tests/zlm_compat_interop.rs`，
手工测试需要在运行服务器后设置 `CHEETAH_SRT_INTEROP=1` 并执行 `cargo test --ignored`。

## 推流

| ID | 客户端 | 命令/步骤 | 期望 | 结果 | 备注 |
|----|--------|-----------|------|------|------|
| P1 | FFmpeg | `ffmpeg -re -stream_loop -1 -i test.ts -c copy -f mpegts "srt://127.0.0.1:9000?streamid=#!::r=live/test,m=publish"` | 流 `live/test` Ready | 待手工 | 依赖运行服务器与 `test.ts` |
| P2 | OBS | URL: `srt://127.0.0.1:9000`, streamid: `#!::r=live/test,m=publish` | 同上 | 待手工 | 无 GUI 测试环境保持手工 |
| P3 | 加密 | 推/拉 passphrase 一致 | 成功 | 待手工 | 可用 `driver-tokio` 加密单测验证底层 |
| P4 | 错误口令 | 推/拉 passphrase 不一致 | 失败 | 待手工 | `driver-tokio` 单测已覆盖 `encryption_passphrase_mismatch_disconnects_caller` |

## 拉流

| ID | 客户端 | 命令/步骤 | 期望 | 结果 | 备注 |
|----|--------|-----------|------|------|------|
| L1 | ffplay | `ffplay -i "srt://127.0.0.1:9000?streamid=#!::r=live/test"` | 可播 | 待手工 | 依赖运行服务器与推送源 |
| L2 | VLC | URL 仅 `srt://127.0.0.1:9000`，偏好设置 streamid=`#!::r=live/test` | 可播 | 待手工 | 无 GUI 测试环境保持手工 |
| L3 | FFmpeg 录像 | `-i srt://127.0.0.1:9000?streamid=#!::r=live/test -c copy out.ts` | `ffprobe` OK | 待手工 | 依赖运行服务器与推送源 |

## 语义负例

| ID | 场景 | 期望 | 验证方式 |
|----|------|------|----------|
| N1 | 仅 `#!::r=live/test`（无 `m`） | **不** 建立 publish 租约；走 play | `module::tests::default_no_m_is_play_or_request` 单元测试 |
| N2 | 双 publish 同 key | 第二路 reject | 待手工 |
| N3 | auth 开 + 错 token | reject:auth | `module::tests::publish_auth_rejects_missing_or_wrong_token` 单元测试 |
| N4 | `#!::r=live` | reject invalid stream id | `module::tests::strict_r_one_segment_fails` 单元测试 |
| N5 | FEC required 无对端 FEC | reject | `module.rs` 在 `Connected` 中已处理 `config.fec.required` 为 `reject:fec_required` |

## 跨协议

| ID | 路径 | 期望 | 结果 |
|----|------|------|------|
| X1 | SRT→RTMP/HLS | 可播或 playlist 有分片 | 待手工 |
| X2 | RTMP→SRT play | ffplay OK | 待手工 |

## 弱网

Phase 02 的 NACK/ARQ 指标与 latency/buffer 配置已通过单元测试覆盖。
弱网 `netem` 复跑需手工环境，记录 metrics 摘要。

## 验收状态

- 代码层 ZLM 语义：已单元测试锁定。
- 外部 P1/L1：待手工环境复跑。
- FEC 对端协商：底层 `shiguredo_srt` 缺少 FEC API，core 纯函数 XOR 已单测通过，
  `fec.required=true` 在 `module.rs` 中已拒绝连接。
