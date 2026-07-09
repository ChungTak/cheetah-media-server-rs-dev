# 点播 / Record 与 ZLMediaKit 差距分析

- **状态**: 已完成
- **范围**: 记录 ZLM 在 MP4、VOD、录制和跨协议控制上的实际行为，本项目现状，以及必须补齐的能力缺口
- **完成标准**: 实现阶段可以按本文逐项补齐 classic MP4、record registry、VOD session、跨协议回放和兼容测试

## ZLM 关键路径

| 领域 | ZLM 文件 | 观察到的行为 |
|------|----------|--------------|
| Recorder factory | `src/Record/Recorder.*` | 按 `Recorder::type` 创建 HLS、MP4、HLS-FMP4、FMP4、TS |
| MP4 recorder | `src/Record/MP4Recorder.*` | 按日期目录写 `.mp4`，临时隐藏文件，关闭后 rename 并广播事件 |
| MP4 muxer | `src/Record/MP4Muxer.*` | 基于 libmov 写 MP4/FMP4，支持 faststart 和 fMP4 segment |
| MP4 demuxer | `src/Record/MP4Demuxer.*` | 支持 track 获取、readFrame、seekTo、多文件串联 |
| MP4 reader | `src/Record/MP4Reader.*` | 读取 MP4 并生成 MediaSource，支持 pause、seek、speed、repeat |
| HLS recorder | `src/Record/HlsRecorder.h` | 同时支持 MPEG-TS HLS 和 HLS-FMP4 |
| FLV recorder | `src/Rtmp/FlvMuxer.*` | 可订阅 RTMP MediaSource 写 FLV 文件 |
| Record API | `server/WebApi.cpp` | `startRecord/stopRecord/isRecording/getMP4RecordFile/deleteRecordDirectory` |
| VOD API | `server/WebApi.cpp` | `loadMP4File` 创建 MP4Reader 并返回 duration |
| RTMP VOD control | `src/Rtmp/RtmpSession.cpp` | 支持 `seek`、`pause`、`onPlayCtrl` speed |
| RTSP VOD control | `src/Rtsp/RtspSession.cpp` | `PLAY Range` seek，`Scale` speed，返回 `RTP-Info` |
| Media control abstraction | `src/Common/MediaSource.h` | `MediaSourceEvent::seekTo/pause/speed/close` |

## ZLM 实际行为判断

ZLM 的 VOD/record 不是单独文件服务，而是把文件和录制器融入 MediaSource 体系：

1. `MP4Reader` 读取文件并创建 `MultiMediaSourceMuxer`
2. RTSP/RTMP 播放端通过 MediaSource 事件控制 seek、pause、speed
3. MP4 录制使用临时隐藏文件，关闭后后台 finalize，再 rename 为正式文件
4. `MultiMP4Demuxer` 支持目录或分号分隔多个 MP4 文件串联为一个 timeline
5. HLS 录制根据 `hls.segNum == 0` 等配置区分直播缓存和录制保留

这意味着 Cheetah 不能只加 MP4 parser，还需要补齐 engine 内的 VOD 控制服务、record task registry 和跨协议控制入口。

## ZLM 标准行为

- MP4 文件读写支持多 track
- MP4 文件播放支持 duration、seek、pause、speed、repeat
- RTSP 通过 `Range: npt=` 和 `Scale` 控制 VOD
- RTMP 通过 `seek`、`pause`、`onPlayCtrl` 控制 VOD
- HLS 和 HLS-FMP4 都可以作为录制输出

## ZLM 落地兼容行为

1. **RTMP `mp4:` URI 修正**  
   VLC、ffplay、mpv 播放 `rtmp://host/record/0.mp4` 时可能发送 `mp4:0` 或 `mp4:0.mp4`，ZLM 会还原成 `0.mp4`。

2. **MP4 录制关闭在后台线程执行**  
   ZLM 认为关闭 MP4 可能耗时，会异步 `closeMP4()`，再统计文件大小、rename、广播事件。

3. **过小 MP4 文件删除**  
   ZLM 对小于 1024 字节的 MP4 录制结果直接删除，避免暴露损坏文件。

4. **seek 后定位关键帧或 config frame**  
   MP4Reader seek 后继续读取，直到找到关键帧或 config frame，再恢复播放时间线。

5. **HLS 按需生成与录制保留共用实现**  
   HLS reader count 会影响是否生成缓存，但录制模式必须持续生成并保留切片。

6. **Record API 使用数字 type**  
   ZLM `startRecord` / `stopRecord` 使用数字 `type`，本地兼容 API 需要支持数字和字符串两种格式。

## 本项目现状

| 能力 | 当前位置 | 状态 |
|------|----------|------|
| 轻量 MP4 sample entry | `crates/foundation/cheetah-codec/src/mp4.rs` | 只有 sample 和 sample entry，不是完整文件读写 |
| fMP4 mux/demux | `cheetah-codec/src/fmp4_mux.rs`、`fmp4_demux.rs` | 已较完整，但只覆盖 fragmented MP4 |
| FLV 协议视图 | `cheetah-codec/src/flv.rs` | 已有封装/解封装主能力 |
| PS mux/demux | `cheetah-codec/src/ps.rs` | 有基础和回归测试，但未文件化 |
| HLS 模块 file output | `crates/protocols/hls/module/` | 有落盘路径，但不是统一 record task |
| MP4 协议 crate | 无 | 完全缺失 |
| 统一 record 模块 | 无 | 完全缺失 |
| VOD API | 无 | 完全缺失 |
| 协议播放 file namespace | 无 | 当前仅 live stream |

## 必须补齐的实现缺口

1. `cheetah-codec` classic MP4 muxer
2. `cheetah-codec` classic MP4 demuxer / index / seek
3. `cheetah-codec` 统一 record writer 事件模型
4. `cheetah-codec` FLV/PS 文件 writer 能力
5. `cheetah-record-module`
6. `cheetah-mp4-core`
7. `cheetah-mp4-driver-tokio`
8. `cheetah-mp4-module`
9. `cheetah-sdk` / `cheetah-engine` 的 `VodControlApi`
10. ZLM 风格 record / loadMP4File / seekRecordStamp / setRecordSpeed API
11. RTMP `mp4:` URI 与 RTSP Range/Scale 兼容
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
- ZLM 的 HLS-FMP4 能力需要在 Cheetah 中和现有 fMP4 muxer 复用，避免复制两套 fragment 逻辑
- FLV record 若只做 classic codec id，会和现有 RTMP/HTTP-FLV 增强 codec 能力脱节
- PS 录制要服从 GB28181 互操作约束，不能把任意 codec 都硬塞进 PS
- 协议模块当前只处理 live stream，file namespace 的 VOD 生命周期必须明确，否则会出现 seek 和 stop 泄漏
