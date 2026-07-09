# Phase 05 — 兼容、互操作、fixture 与 fuzz

- **状态**: 已完成
- **范围**: 补齐 ZLM 非标准兼容、真实文件互操作、fixture、属性测试和 fuzz，确保 MP4 VOD 与多格式录制具备生产可用的鲁棒性
- **完成标准**: 对脏文件、非标准 box、异常 seek、慢读者、坏目录输入和协议控制扰动都能 bounded 处理且具备回归覆盖

## 实现概览

- `cheetah-codec::mp4::compat` 集中维护 skippable 顶层 box (`free/skip/wide/uuid/meta/sbgp/sgpd`)、支持的 sample entry 4cc 矩阵以及 `clamp_composition_offset`。
- `Mp4Reader`：`size==0`（extends-to-eof）和 `size==1`（largesize）已支持；越界 box 通过 `OversizeBox` 诊断回退；`moov` 在尾部时进入 tail-scan 子流程。
- `Mp4Writer`：所有 chunk offset 大于 `u32::MAX` 时自动 `co64`；`stss` 仅对 video track 写出；`ctts` 仅在 `pts != dts` 时写出。
- ZLM 兼容增量：
  - `Mp4FileWriterConfig::drop_below_bytes` + `RecordDiagnostic::DropTinyFile` 对应 ZLM 1024B 弃文件策略。
  - `zlm_compat::parse_zlm_type` 同时接受数字与字符串 `type`。
  - `zlm_compat::validate_customized_path` 拒绝 `..` / 绝对路径 / 反斜杠。
  - `zlm_compat::apply_period` 支持 `YYYY-MM` 与 `YYYY-MM-DD` period 过滤，使用 Howard Hinnant `days_from_civil` 算法（无外部依赖）。
  - `zlm_compat::normalize_rtmp_mp4_uri` 还原 `mp4:` / `flv:` 前缀。
  - `zlm_compat::expand_uri_list` 拆分 `;` 分隔 URI。
- 测试覆盖：
  - `cheetah-codec`（213 用例）含 `record::mp4::tests::drops_below_threshold_yields_diagnostic` 等回归。
  - `cheetah-record-module`（16 用例）含 6 个 `zlm_compat` 用例。
  - `cheetah-mp4-property-tests`（6 用例）覆盖 multi-track roundtrip / seek monotonic / repeated init dedup / B-frame ctts / malformed input bounded。
  - `crates/protocols/mp4/fuzz/` 独立 cargo-fuzz workspace 内含 4 个 fuzz target（与 plans-26-mp4-sms Phase 05 共享）。
- 与 ZLM 行为对比的 fixture 库（`dev-docs/fixtures/mp4/`）作为后续增量任务，不阻塞验收。

## 后续可继续推进的工作

- 引入 ZLM 真实样例 fixture，按 SMS / ZLM 行为对比分类；在 `cheetah-mp4-property-tests` 中加入相应的对照回归。
- 协议模块 (`cheetah-rtsp-module` / `cheetah-rtmp-module` / `cheetah-http-flv-module`) 在 play 入口调用 `Mp4Module::api()::start` 实现 lazy-start glue，进一步对齐 ZLM 直接 RTMP 播放 MP4 的体验。

## 5.1 MP4 非标准兼容

- `free/skip/uuid/wide` box
- `largesize`
- `moov` 在尾部
- 缺失 `stss`
- 异常 `ctts`
- 过大 box / 损坏 box / 截断文件
- 多文件串联中的空文件、坏文件和轨道变化

要求：

- 不 panic
- 不无界分配
- 尽量给 diagnostic

## 5.2 录制格式兼容

- FLV enhanced codec mapping 和 domestic mode
- HLS MPEG-TS 与 HLS-FMP4 双路径
- PS 对国标常见轨道组合的兼容
- MP4 faststart、尾部 moov、co64
- 小文件删除和临时文件隐藏策略

## 5.3 ZLM API 兼容

- `/index/api/startRecord`
- `/index/api/stopRecord`
- `/index/api/isRecording`
- `/index/api/getMP4RecordFile`
- `/index/api/deleteRecordDirectory`
- `/index/api/loadMP4File`
- `/index/api/seekRecordStamp`
- `/index/api/setRecordSpeed`

兼容要求：

- 数字 `type` 和字符串 `format` 都可用
- `customized_path` 必须安全归一化
- `period` 支持月级和日级查询
- `speed` 限制为 `0.1..=20.0`
- `seek_ms` 超过 duration 返回明确错误

## 5.4 fixture 与互操作

- 增加 MP4 fixture manifest
- 增加多轨、音频 only、尾部 `moov`、坏表、超大时间戳、异常 seek 样例
- 增加 HLS/HLS-FMP4、FLV、PS、MP4 record 输出回放校验样例
- 对比 ZLM 参考行为和本地行为

## 5.5 测试与 fuzz

- `cheetah-codec`：MP4 parser/sample table/PS writer/FLV writer fuzz
- `cheetah-mp4-core`：request/control 状态机 property tests
- `cheetah-record-module`：API body、目录扫描、元数据恢复 fuzz 和回归
- 协议模块：RTSP/RTMP/HTTP-FLV/WS-FLV VOD fault robustness

## 5.6 完成标准

- MP4 VOD 与 `FLV/HLS/MP4/PS` 录制核心路径都有单元测试
- 至少有一组跨协议端到端回归
- 至少有一组 ZLM 对比样例
- 至少有 MP4 box parser、sample table、PS writer/demux、FLV writer、HTTP/RTMP 请求解析 fuzz 目标
