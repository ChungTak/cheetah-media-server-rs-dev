# TS ABL 对照差距分析

- **状态**: 计划中
- **范围**: 对照 ABLMediaServer 版本信息和 TS 相关源码，识别本地 TS 下一步应补齐的标准与非标准能力。

---

## 版本信息结论

`版本信息.txt` 中与 TS/传输兼容最相关的变化：

| 日期 | 主题 | 对 TS 的启发 |
|------|------|--------------|
| 2026-05-08 | 优化 RTP 接收缓冲区切割，兼容海康下级平台发送的国标流 | RTP payload 不应假设一次 read 就是完整 TS/PS，需要按 RTP header、payload type、SSRC 和 TS sync 做二次校验 |
| 2026-04-27 | 运行时开关 FFmpeg 日志 | TS pull/interop 测试需要可控诊断级别，避免噪声掩盖协议错误 |
| 2026-04-24 | 国标接入和发送修正真实帧速度计算 | TS/RTP 输入不能固定 25fps；应从 RTP timestamp 或 demux PTS 统计真实帧率 |
| 2026-04-23 | 统计帧速度窗口调整，最小断开超时保护 | 需要冷路径状态统计，避免设备首帧慢或低帧率被误判失败 |
| 2026-03-27 | 1078 帧速度统计增至 500 帧，MP4 按真实帧率写入 | 长窗口平均能提升真实设备稳定性；记录/回放/TS 输出都应使用媒体真实时间 |
| 2026-03-24 | RTMP/RTP 帧速度采用 5 秒平均 | 本地可做独立 `FrameRateEstimator`，避免各 module 复制算法 |
| 2026-03-21 | Linux/ARM 补 `htonll`/`ntohll` | RTP/TS 解析测试应覆盖跨平台字节序和 64-bit timestamp helper |

结论：ABL 的主要价值不在 TS 标准语法本身，而在真实设备输入的容错、帧率/时间戳推断、国标 RTP 接入和发送链路的运行稳定性。

---

## ABL 源码观察

### HTTP-TS 输出

参考 `NetServerHTTP_TS.cpp`：

1. 通过 `mpeg_ts_create()` 和 `mpeg_ts_write()` 按需生成 TS。
2. `http_ts_record_ts_write()` 将 188-byte packet 先拼进 `pSendTsCacheBuffer`，接近上限再批量 `SendTSBufferData()`。
3. 响应头使用 `Content-Type: video/mp2t; charset=utf-8`、`Connection: keep-alive`、CORS 和 `Keep-Alive`。
4. `SendTSBufferData()` 将大块输出切成 `Send_TSMedia_MaxPacketCount` 发送，写错误累计到阈值后断开。
5. H264/H265 通过 I-frame 检测设置 random access flag。
6. 音频主要处理 AAC/MP3，代码中也保留 G711A/G711U stream_type 分支。
7. 录制回放帧前带 frame number，live 与 replay 的 DTS 推进路径不同。

### WS-TS 输出

参考 `NetServerWS_TS.cpp`：

1. 与 HTTP-TS 共享 `mpeg_ts_write()` 思路。
2. TS packet 先批量缓存，再通过 WebSocket 发送。
3. 维护 WebSocket 握手状态、`Sec-WebSocket-Key`、`Sec-WebSocket-Protocol` 和写错误计数。
4. 支持 mute packet list，用于新连接或音频缺失场景的静音补偿。

### RTP-TS 输入

参考 `NetServerRecvRtpTS_PS.cpp` 与 `RtpTSStreamInput.cpp`：

1. UDP 收到 RTP 后用 SSRC 派生 client key。
2. 通过 RTP payload 开头判断 PS 头或 TS 头，创建不同输入对象。
3. `RtpTSStreamInput::InputNetData()` 要求 `nDataLength >= 12` 且 `(nDataLength - 12) % 188 == 0`。
4. 每个 RTP 包跳过 12-byte RTP header 后按 188 字节喂给 `ts_demuxer_input()`。
5. demux callback 将 H264/H265/AAC 推入 media source，AAC 会解析 ADTS 以取得 channels/sample_rate。
6. 对 G711A/G711U 有分支，但原代码没有完整推音频数据，属于本地实现时需要补完的兼容点。
7. 媒体源首次到达时创建，之后用 `CalcVideoFrameSpeed()` 更新真实帧率。

---

## 本地差距表

| 能力 | 本地状态 | ABL 对照缺口 | 计划 |
|------|----------|--------------|------|
| HTTP-TS live | 已有 HTTP/HTTPS 输出 | 缺少 ABL 风格批量 flush、写错误计数、`video/mp2t; charset=utf-8` 兼容测试 | Phase 03 |
| WS-TS live | 已有 WS/WSS binary 输出 | 服务端未完整读取客户端 ping/close/text 帧；缺少 payload 上限配置落地 | Phase 03 |
| HTTP(S)-TS pull | 已有 200/206、空 body 失败 | HTTP chunked 解码需要确认实现；Content-Type warn-only 未形成可配置诊断 | Phase 03 |
| WS(S)-TS pull | 已有 binary frame 读取 | WebSocket key 固定，不验证 `Sec-WebSocket-Accept`，ping pong 长 payload 分支薄弱 | Phase 03 |
| RTP-TS 输入 | 未见独立 ts RTP driver/module | 缺 SSRC 分流、PS/TS 自动识别、RTP header 校验、切包容错 | Phase 02 |
| RTP 接收切割兼容 | codec demux 可任意 bytes | RTP 层仍需要处理 header extension、CSRC、padding、marker、非 12-byte header | Phase 02 |
| 真实帧率估计 | 未见 TS/RTP 专用 estimator | ABL 通过 250/500 帧窗口或 5 秒平均修正帧率 | Phase 01/02 |
| G711 时间戳 | codec 有 stream_type | 需要按 payload length/sample_rate 推导 duration，避免固定帧率 | Phase 01 |
| AAC ADTS | mux raw AAC 会包装，demux 有 ADTS 推导 | 需要补连续 ADTS frame split、异常 ADTS 长度诊断样例 | Phase 01 |
| 多轨道 | mux/demux 有雏形 | 需要 track 排序、增量 update、超限 diagnostic 和多节目策略 | Phase 04 |
| 非标准编码 | stream_type 已覆盖大半 | 需要 ABL/libmpeg fixtures 验证 OPUS/G711/VPx/AV1/MP2 | Phase 04 |
| mute/silence 补偿 | 未见 TS module 逻辑 | ABL WS 侧有静音包列表；本地可只做可选 audio gap filler | Phase 04 |
| TS 录制/回放 | 不在当前 TS live 主路径 | ABL 有 `StreamRecordTS` 与 replay frame number；本轮记录为后续，不阻塞 live | 后续 |

---

## 本轮不做

1. 不实现 ABL 的私有 SDK、FFmpeg 转码、MP4 文件循环读取和 TS 录制完整功能。
2. 不在 TS module 中复制 codec 层时间戳、ADTS、参数集缓存逻辑。
3. 不把 RTP-TS 输入混入现有 HTTP/WS driver；它应是独立 driver 能力或 TS module pull source 类型。
4. 不支持无限缓存；所有 RTP 重组、PES 重组、WebSocket frame、写队列都有上界。
