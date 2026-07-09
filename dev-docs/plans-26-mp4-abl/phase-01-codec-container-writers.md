# Phase 01: `cheetah-codec` 容器与 Writer 能力补齐

- **状态**: 已完成
- **目标**: 把 MP4 点播和多格式录制的公共媒体能力收敛到 `cheetah-codec`
- **完成标准**: classic MP4、FLV、HLS、PS、TS、FMP4 的读写抽象、时间戳与 compat 策略可供 record/VOD 复用

## 实现概览

- 共用 `plans-26-mp4-sms` Phase 01 的 `cheetah-codec::mp4`（box parser / sample table / sample entry / writer / reader / compat）与 `cheetah-codec::record` 容器接口（`flv/hls/mp4/ps`）。
- 真实帧率估算复用 `cheetah-codec::egress::FrameRateEstimator`：录制 / VOD / PS 写出统一通过该估算器，避免再次出现 ABL 历史中“固定 25fps”的问题。
- ABL 关注的容器细节均已落地：
  - `Mp4Reader` 接受 `moov` 在前或在后，且越界 box 走 `OversizeBox` 诊断回退。
  - `Mp4Writer` 在 chunk offset 超过 `u32::MAX` 时切换到 `co64`。
  - `parse_ctts` 区分 v0 (unsigned) 与 v1 (signed)，并通过 `compat::clamp_composition_offset` 防止 i32 溢出。
  - 时间戳辅助路径用 i128 中间值避免溢出。
- 测试覆盖：`cargo test -p cheetah-codec --lib` 213 用例通过。

## 交付项

1. classic MP4 reader，支持 box 解析、sample table、chunk、sync sample、时间线索引
2. classic MP4 writer，支持 `ftyp/moov/moof?` 区分、track metadata、sample 写入与 finalize
3. 统一 `RecordWriter` trait，封装 `Mp4Writer`、`FlvWriter`、`HlsWriter`、`PsWriter`
4. 真实帧率估算器，按窗口样本统计视频帧速度，避免固定 25fps
5. 时间戳修正器，统一处理 DTS/PTS、音频持续时长、缺失时间戳和 seek 后重基准

## ABL 对齐要求

1. AAC 支持 ADTS 缺失补齐
2. G711 按样本数和采样率推导持续时长
3. H264/H265 支持参数集缓存和 seek/切片后的首包补发
4. MP4 文件读取时支持 `moov` 前置或后置、`free/skip/wide/uuid` 等非关键 box
5. 对损坏 sample table、异常 `ctts`、缺失 `stss` 采用 bounded 失败或降级 seek

## 设计要点

1. `TrackInfo` 是唯一轨道元数据入口，不在 writer 层复制私有 codec 描述结构
2. writer 输入统一为 `Arc<AVFrame>`，避免录制路径重复转封装
3. `Mp4Index` 明确拆分为 sample、chunk、time-to-sample、composition offset、sync sample
4. 真实帧率估算器应能被 record、VOD 和 PS 输出复用
5. compat 逻辑集中在 `codec::compat` 或 `codec::record` 子模块，不散落在协议 crate

## 测试要求

1. MP4 box parser 单元测试覆盖正常 box、`largesize`、未知 box、损坏 box
2. sample table 属性测试覆盖时间线递增、chunk 偏移合法性和 sync sample 边界
3. MP4 reader/writer 回环测试覆盖单轨、多轨、仅视频、仅音频
4. AAC/G711/H264/H265 兼容测试覆盖 ADTS、参数集、时间戳推导
5. fuzz 覆盖 MP4 box parser、sample table parser、writer finalize 输入
