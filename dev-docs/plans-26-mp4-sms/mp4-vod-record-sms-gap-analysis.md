# MP4 / Record 与 SimpleMediaServer 差距分析

- **状态**: 已完成
- **范围**: 记录 SMS 在 MP4、VOD、录制和文件回放上的实际行为，本项目现状，以及必须补齐的能力缺口
- **完成标准**: 实现阶段可以按本文逐项补齐 classic MP4、record registry、VOD session、跨协议回放和兼容测试

## SMS 关键路径

| 领域 | SMS 文件 | 观察到的行为 |
|------|----------|--------------|
| MP4 muxer | `Mp4/Mp4Muxer.*` | 写 `ftyp/mdat/moov`，构造 sample table，支持 `co64`、`ctts`、`stss` |
| MP4 demuxer | `Mp4/Mp4Demuxer.*` | 解析 classic MP4 box，支持 `mov_reader_seek`、duration、track info |
| MP4 file writer | `Mp4/Mp4FileWriter.*` | 面向录制文件的落盘封装 |
| MP4 file reader | `Mp4/Mp4FileReader.*` | 面向 VOD 的读取、解包和 frame 回调 |
| Record registry | `Record/Record.*` | 统一管理录制任务、task id、record template |
| FLV record | `Record/RecordFlv.*` | 订阅源流并写 FLV |
| HLS record | `Record/RecordHls.*` | 订阅源流并写 HLS segment/playlist |
| MP4 record | `Record/RecordMp4.*` | 订阅源流并写 MP4 |
| PS record | `Record/RecordPs.*` | 订阅源流并写 PS |
| VOD API | `Api/VodApi.cpp` | `start/control/stop`，控制 seek/pause/scale |
| Record API | `Api/RecordApi.cpp` | `start/list/stop/query/file/query/file/delete` |
| Record reader | `Record/RecordReader*.{h,cpp}` | 抽象回放控制器，支持 mp4/flv/ps/dir/record 等 |

## SMS 实际行为判断

从 `vendor-ref/simple-media-server/Src` 看，SMS 的 VOD/record 重点在“媒体文件 I/O + 控制 API + media source 桥接”，不是单纯容器工具库：

1. 文件回放通过 reader 控制 `seek/pause/scale/stop`
2. 录制通过统一 registry 管理 task id 和元数据
3. 文件格式扩展点在 `Record` / `RecordReader` 抽象，而不是协议模块内部 if-else
4. RTSP/RTMP 等外放协议消费的是统一 media source，而不是每个协议各自读文件

这意味着本地实现不能只加一个 MP4 parser，还要同时补齐任务模型、文件索引和跨协议桥接。

## SMS 标准行为

- VOD 支持 `seek`、`pause`、`scale`
- 录制支持按 `duration`、`segmentDuration`、`segmentCount` 控制
- MP4 reader 提供 duration、first dts、seek 和 paced frame 输出
- 文件与记录查询 API 返回可枚举的 record file 列表

## SMS 落地兼容行为

1. **VOD URI 带 `file/record/dir` 前缀**  
   SMS 通过 URI namespace 区分文件 VOD、录像回放、目录回放。本地也要保留 `file/` 和 `record/` 兼容入口。

2. **seek/pause/scale 由控制 API 和协议控制共同触发**  
   `VodApi.cpp` 既可独立调用，也可被播放链路复用。本地也应统一到单一 `VodControlApi`。

3. **录制格式通过字符串枚举扩展**  
   `RecordApi.cpp` 用 `format` 决定 writer 类型。本地需要 registry 式设计，不能把格式分支散落在模块入口。

4. **MP4 录制切片以关键帧边界为优先**  
   `RecordMp4` 在 key/meta frame 处切新文件。本地应保持相同行为，避免不可解码切片。

5. **记录文件以日期目录组织**  
   SMS 把文件按天落到目录，本地也应保持相近布局，便于 list/query/delete。

## 本项目现状

| 能力 | 当前位置 | 状态 |
|------|----------|------|
| 轻量 MP4 sample entry | `crates/foundation/cheetah-codec/src/mp4.rs` | 只有 sample 和 sample entry，不是完整文件读写 |
| fMP4 mux/demux | `cheetah-codec/src/fmp4_mux.rs`、`fmp4_demux.rs` | 已较完整，但只覆盖 fragmented MP4 |
| FLV 协议视图 | `cheetah-codec/src/flv.rs` | 已有封装/解封装主能力 |
| PS mux/demux | `cheetah-codec/src/ps.rs` | 已有基础和回归测试，但未文件化 |
| HLS 模块 file output | `crates/protocols/hls/module/` | 有落盘路径，但不是统一 record task |
| MP4 协议 crate | 无 | 完全缺失 |
| 统一 record 模块 | 无 | 完全缺失 |
| VOD API | 无 | 完全缺失 |
| 协议播放 file namespace | 无 | 当前仅 live stream |

## 必须补齐的实现缺口

1. `cheetah-codec` classic MP4 muxer
2. `cheetah-codec` classic MP4 demuxer / index / seek
3. `cheetah-codec` 统一 record writer 事件模型
4. `cheetah-codec` PS 文件 writer 能力
5. `cheetah-record-module`
6. `cheetah-mp4-core`
7. `cheetah-mp4-driver-tokio`
8. `cheetah-mp4-module`
9. `cheetah-sdk` / `cheetah-engine` 的 `VodControlApi`
10. SMS 风格 `vod` / `record` 控制 API
11. file/record namespace 与跨协议 seek 控制
12. 文件元数据、目录查询、删除、回放 URL 生成

## 编码矩阵判断

| 编码 | MP4 record | HLS record | FLV record | PS record | MP4 VOD |
|------|------------|------------|------------|-----------|---------|
| H264 | 支持 | 支持 | 支持 | 支持 | 支持 |
| H265 | 支持 | 支持 | 支持 | 支持 | 支持 |
| AAC | 支持 | 支持 | 支持 | 支持 | 支持 |
| G711A/U | 支持 | 支持 | 支持 | 支持 | 支持 |
| Opus | 支持 | 支持 | 支持 | 不作为主路径 | 支持 |
| MP3 | 支持 | 支持 | 支持 | 支持 | 支持 |
| VP8 | 支持 | 支持 | 兼容扩展 | 不作为主路径 | 支持 |
| VP9 | 支持 | 支持 | 兼容扩展 | 不作为主路径 | 支持 |
| AV1 | 支持 | 支持 | 兼容扩展 | 不作为主路径 | 支持 |

## 互操作风险

- classic MP4 sample table 和 seek 是最大实现风险，不是现有 fMP4 能力的小修
- HLS 录制若继续只走 TS，会损失 `Opus/VP8/VP9/AV1` 覆盖
- FLV record 若只做 classic codec id，会和现有 RTMP/HTTP-FLV 增强 codec 能力脱节
- PS 录制要服从 GB28181 互操作约束，不能把任意 codec 都硬塞进 PS
- 协议模块当前只处理 live stream，file namespace 的 VOD 生命周期必须明确，否则会出现 seek 和 stop 泄漏
