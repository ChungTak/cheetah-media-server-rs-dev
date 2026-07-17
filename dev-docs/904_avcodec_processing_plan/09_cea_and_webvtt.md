# 09 · CEA 与 WebVTT

## 1. 分层

CEA 提取不需要像素解码，放在 `cheetah-codec` 的纯 Sans-I/O compat/parser 层：

```text
H.264/H.265 Access Unit
  -> SEI NALU
  -> user_data_registered_itu_t_t35
  -> ATSC GA94 / type 0x03
  -> cc_data packets
  -> CEA-608/708 state machine
  -> normalized WebVTT cues
```

parser 不读系统时间、不持有 EngineContext、不发布流。`CaptionExtract` Job 负责订阅源、驱动 parser、建立字幕 TrackInfo 并发布目标派生流。

## 2. SUB-01：公共媒体模型

- `CodecId::WebVtt`
- `MediaKind::Subtitle`
- UTF-8 cue payload
- PTS 表示 cue start，duration 表示 cue end-start
- TrackInfo metadata 包含 language、CEA service/channel 和 reference video track

字幕输出与目标视频/音频共享一个派生 publisher 的多轨流；没有视频复制/转码要求时允许 passthrough 原媒体轨并新增字幕轨。

## 3. Parser 合同

- H.264 解析 SEI type 4；H.265 解析 prefix/suffix SEI 中 registered user data。
- 校验 country/provider/user identifier/type code、process flag、cc_count 和 marker bits。
- 支持 CEA-608 CC1–CC4 和 CEA-708 service 1–63；默认 CC1/service 1，配置只能选择已发现服务。
- 实现 pop-on、paint-on、roll-up、clear、backspace、carriage return 和基本样式/定位。
- 未支持控制码保留 diagnostic，不得 panic 或输出乱码。
- cue 文本规范化为 UTF-8，重复屏幕状态不重复发 cue。
- discontinuity/seek/reset 关闭当前 cue、清空 decoder 状态，不跨时间线连接字幕。
- 单 Access Unit、单 packet、单 cue 文本、窗口和 pending state 全部有上限。

## 4. HLS WebVTT

新增 HLS `VttMuxer`：

- 使用视频 reference track 的 segment/part 边界切分 cue。
- 跨 segment cue 在相关 VTT segment 中生成正确 local timing，不丢失持续显示。
- VTT segment 以 `WEBVTT` 开头，时间戳单调。
- 生成独立 subtitle media playlist。
- master playlist 增加 `EXT-X-MEDIA:TYPE=SUBTITLES` 和 variant 的 `SUBTITLES` group。
- subtitle playlist/segment 使用现有鉴权、Cache-Control、内存/文件生命周期和 session 规则。

本期不向 RTMP/HTTP-FLV 强行写入字幕；这些协议仅能消费原有 data track 兼容路径。能力报告按协议分别声明。

## 5. 测试

- 608/708 标准和真实 GA94 fixture，覆盖 H.264/H.265、多个 service、控制码和断流。
- parser 单元测试、属性测试和 fuzz：任意 NALU/SEI 不 panic，内存有界，错误可诊断。
- cue golden 验证文字、start/end、重复抑制、roll-up 和 reset。
- HLS 验证 VTT segment、跨分片 cue、subtitle playlist、master group 和 URL 鉴权。
- 用支持字幕的 HLS 客户端验证可选择字幕、切换 ABR 不丢字幕。
- Caption Job stop/restart/source reconnect 后无悬挂 cue、publisher 或 parser state。

## 6. 完成标准

- [ ] CEA parser 完全 Sans-I/O，不依赖 avcodec session、runtime 或协议模块。
- [ ] WebVTT 作为正式 Subtitle track 进入 `AVFrame + TrackInfo` 模型。
- [ ] HLS 输出可由独立 parser/player 验证，不只检查字符串存在。
- [ ] 不支持的服务、控制码和协议出口返回明确 diagnostic。
