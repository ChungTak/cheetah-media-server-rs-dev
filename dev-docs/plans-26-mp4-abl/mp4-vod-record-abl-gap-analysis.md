# 点播 / Record 与 ABLMediaServer 差距分析

- **状态**: 已完成
- **范围**: 记录 ABL 在 MP4、录制、回放和跨协议控制上的实际行为，本项目现状，以及必须补齐的缺口

## ABL 关键路径

| 领域 | ABL 文件 | 观察到的行为 |
|------|----------|--------------|
| 本地 MP4 回放 | `NetClientReadLocalMediaFile.*` | 支持文件读取、pause、resume、seek、scale、循环回放 |
| 多文件回放 | `NetServerReadMultRecordFile.*` | 支持多录制文件拼接回放 |
| 文件索引 | `RecordFileSource.*` | 维护录制文件列表、m3u8 缓存和过期清理 |
| MP4 录制 | `StreamRecordMP4.*` | MP4 文件录制与收尾 |
| FMP4 录制 | `StreamRecordFMP4.*` | fMP4 切片录制 |
| TS/PS 录制 | `StreamRecordTS.*`、`StreamRecordPS.*` | MPEG-TS / PS 文件输出 |
| HTTP-MP4 | `NetServerHTTP_MP4.*` | 支持 HTTP-MP4 回放和 chunked 输出 |
| HLS | `NetServerHLS.*` | 录制和回放共用媒体源切片思路 |
| FLV | `NetServerHTTP_FLV.*`、`NetServerWS_FLV.*` | HTTP-FLV / WS-FLV 文件回放 |

## 从 `版本信息.txt` 抽取的关键行为

### 回放控制

1. `readMp4FileCount` 控制本地 MP4 文件播放次数，默认 1 次，`-1` 无限循环
2. seek 使用 `av_seek_frame(..., AVSEEK_FLAG_BACKWARD)` 回到最近关键位置
3. 暂停状态下会阻止部分 seek 行为，说明控制状态是有约束的
4. 8x、16x 回放只发送 I 帧，属于显式的高倍速退化策略

### 时间戳与帧率

1. ABL 多次修正“真实帧率”计算，覆盖 RTP、RTMP、1078、ES 和 MP4 录制
2. 录制 MP4 不能固定以 25fps 写入，需要依据样本统计结果更新
3. PS 发送和文件写出也依赖真实帧率，说明帧率估算应为共享能力

### 协议输出

1. 录制回放覆盖 `RTSP`、`RTMP`、`HTTP-FLV`、`WS-FLV`、`HTTP-MP4`、`HTTP-TS`、`WS-TS`
2. `on_rtsp_replay` 等控制事件附带 `readerCount`、`ip`、`port`、`networkType`、`params`
3. HTTP-MP4 使用 chunked 传输，并要求大块数据拆分发送

### 文件与目录

1. 支持直接把 `d:\\video\\x.mp4` 或 `/home/video/x.mp4` 作为回放源
2. HLS 回放不再简单拼接历史 m3u8，而是复用媒体源和内存切片逻辑
3. 录制文件及 m3u8 有过期清理语义，说明 record catalog 不能只是目录扫描

## 本项目现状

| 能力 | 当前位置 | 状态 |
|------|----------|------|
| classic MP4 读写 | `crates/foundation/cheetah-codec/src/mp4/` | 已有基础，但未完整对齐 VOD/record 需要 |
| fMP4 mux/demux | `cheetah-codec` | 已有 live 基础 |
| FLV/PS/TS 基础封装 | `cheetah-codec` | 有协议视图，无统一录制 writer |
| HLS module 落盘 | `crates/protocols/hls/module/` | 有局部能力，未系统化 |
| 统一 record module | `crates/system/cheetah-record-module/` | 存在进行中的工作，但未形成 ABL 对齐设计文档 |
| MP4 协议三段式 | `crates/protocols/mp4/` | 存在进行中的工作，需补齐边界与兼容点 |
| 协议级 VOD 控制 | `rtsp/rtmp/http-flv` | 仍以 live stream 为主 |

## 必须补齐的实现缺口

1. `cheetah-codec` 的 classic MP4 reader、writer、seek index 和多轨 support
2. `cheetah-codec` 的真实帧率估算器和统一时间戳修正器
3. `cheetah-record-module` 的多格式 writer registry、文件 catalog 和事件模型
4. `cheetah-mp4-core` 的 seek/pause/resume/speed/read_count 状态机
5. `cheetah-mp4-driver-tokio` 的文件任务池、循环回放和多文件串联
6. `cheetah-mp4-module` 的 VOD source 和跨协议控制映射
7. RTSP/RTMP/HTTP-FLV/WS-FLV 的文件 namespace、seek、pause、speed
8. ABL 兼容 profile，包括高倍速关键帧回放、seek 越界报错、chunked HTTP-MP4 和 replay webhook 字段

## 主要风险

1. classic MP4 sample table 和 seek 是主风险，不是现有 fMP4 能力的小修
2. 多文件回放需要统一时间线，否则 seek 和 duration 会失真
3. 真实帧率估算如果分散到各协议实现，会再次复制 ABL 历史问题
4. HTTP-FLV / WS-FLV 回放若不复用统一 VOD source，会与 RTSP/RTMP 行为偏离
