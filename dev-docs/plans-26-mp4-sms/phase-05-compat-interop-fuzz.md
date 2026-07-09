# Phase 05 — 兼容、互操作、fixture 与 fuzz

- **状态**: 已完成
- **范围**: 补齐 SMS 非标准兼容、真实文件互操作、fixture、属性测试和 fuzz，确保 MP4 VOD 与多格式录制具备生产可用的鲁棒性
- **完成标准**: 对脏文件、非标准 box、异常 seek、慢读者、坏目录输入和协议控制扰动都能 bounded 处理且具备回归覆盖

## 实现概览

- `cheetah-codec::mp4::compat` 集中维护 skippable 顶层 box (`free/skip/wide/uuid/meta/sbgp/sgpd`)、支持的 sample entry 4cc 矩阵以及 `clamp_composition_offset` 等帮助函数。
- `Mp4Reader`：`size==0`（extends-to-eof）和 `size==1`（largesize）已支持；越界 box 通过 `OversizeBox` 诊断回退；`moov` 在尾部时进入 tail-scan 子流程。
- `Mp4Writer`：当所有 chunk offset 大于 `u32::MAX` 时自动切换到 `co64`；`stss` 仅对 video track 写出；`ctts` 仅在出现 `pts != dts` 时写出。
- 新增 crate `cheetah-mp4-property-tests`，覆盖：
  - writer/reader 多轨 roundtrip 帧计数一致
  - VOD seek 后视频时间戳不回退（除显式 seek）
  - 重复 init 不重复发布 track 列表
  - 含/不含 B 帧时 `ctts` 行为
  - 损坏 box (`size==0` + 后续越界) 在 bounded 步数内退出
- 与 `RecordContainerWriter` 配套的 FLV/HLS/MP4/PS 单元测试在 `cheetah-codec::record::*` 内。
- `cargo test -p cheetah-codec --lib`（212 用例）+ `cheetah-record-module`（10 用例）+ `cheetah-mp4-core`（4 用例）+ `cheetah-mp4-driver-tokio`（1 集成用例）+ `cheetah-mp4-module`（3 用例）+ `cheetah-mp4-property-tests`（6 用例）全部通过。

## 后续可继续推进的工作

- `crates/protocols/mp4/fuzz/` cargo-fuzz workspace 已落地，包含四个 fuzz target（`fuzz_mp4_box_parser` / `fuzz_mp4_sample_table` / `fuzz_mp4_reader_dirty` / `fuzz_mp4_vod_session`）。该 crate 保持独立 workspace，不在根 members 中；运行方式：`(cd crates/protocols/mp4/fuzz && cargo +nightly fuzz run fuzz_mp4_box_parser)`。
- 在真实设备/SMS 互操作层面追加 fixture 库；当前 fixture 体系建议放在 `dev-docs/fixtures/mp4/` 下，按 SMS 行为分类。

## 5.1 MP4 非标准兼容

- `free/skip/uuid/wide` box
- `largesize`
- `moov` 在尾部
- 缺失 `stss`
- 异常 `ctts`
- 过大 box / 损坏 box / 截断文件

要求：

- 不 panic
- 不无界分配
- 尽量给 diagnostic

## 5.2 录制格式兼容

- FLV enhanced codec mapping 和 domestic mode
- HLS fMP4/TS 双路径
- PS 对国标常见轨道组合的兼容
- MP4 faststart、尾部 moov、co64

## 5.3 fixture 与互操作

- 增加 MP4 fixture manifest
- 增加多轨、音频 only、尾部 `moov`、坏表、超大时间戳、异常 seek 样例
- 增加 record 输出回放校验样例
- 对比 SMS 参考行为和本地行为

## 5.4 测试与 fuzz

- `cheetah-codec`：MP4 parser/sample table/PS writer fuzz
- `cheetah-mp4-core`：request/control 状态机 property tests
- `cheetah-record-module`：API body、目录扫描、元数据恢复 fuzz 和回归
- 协议模块：RTSP/RTMP/HTTP-FLV/WS-FLV VOD fault robustness

## 5.5 完成标准

- MP4 VOD 与 `FLV/HLS/MP4/PS` 录制核心路径都有单元测试
- 至少有一组跨协议端到端回归
- 至少有一组 SMS 对比样例
- 至少有 MP4 box parser、sample table、PS writer/demux、HTTP/RTMP 请求解析 fuzz 目标
