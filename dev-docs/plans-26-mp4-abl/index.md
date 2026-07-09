# 点播与多格式录制实现计划（对标 ABLMediaServer）

- **状态**: 已完成
- **目标**: 新增 MP4 文件点播与统一录制能力，对齐 ABLMediaServer 在本地文件回放、录制回放、seek、pause、scale、多文件回放和跨协议输出上的工程行为
- **方法**: 参考 `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer` 与 `vendor-ref/ABLMediaServer-src-2026-05-09/版本信息.txt`，结合本项目现有 `cheetah-codec`、`cheetah-fmp4-*`、`cheetah-hls-*`、`cheetah-rtmp-*`、`cheetah-rtsp-*`、`cheetah-http-flv-*`
- **完成标准**: MP4 VOD、统一录制任务、ABL 风格控制接口、非标准兼容、单元测试、属性测试、fuzz 和跨协议集成验证全部形成实现设计

---

## V1 范围

首版固定支持：

1. MP4 文件点播，覆盖 `RTSP`、`RTMP`、`HTTP-FLV`、`WS-FLV`
2. 点播控制支持 `start`、`seek`、`pause`、`resume`、`speed`、`stop`
3. 统一录制任务，首批输出 `FLV`、`HLS`、`MP4`、`PS`
4. 录制格式 registry 预留 `FMP4`、`TS` 扩展位，与 ABL 的 `HTTP-MP4`、`HTTP-TS`、`WS-TS` 回放能力对齐
5. 多轨道模式，媒体统一收敛为 `AVFrame + TrackInfo`
6. ABL 风格 `readMp4FileCount`、循环回放、真实帧率、seek 越界错误和高倍速关键帧回放兼容
7. 单元测试、集成测试、属性测试和 fuzz 测试

首版分期约束：

1. **Phase 01** 先补 `cheetah-codec` 的 MP4 与录制容器能力
2. **Phase 02** 再补统一录制模块和文件元数据管理
3. **Phase 03** 建立 MP4 VOD 三段式 crate
4. **Phase 04** 接入 `RTSP/RTMP/HTTP-FLV/WS-FLV`
5. **Phase 05** 补齐 ABL 非标准兼容、互操作和 fuzz/fixture 体系

首版不做：

1. 转码；目标协议或目标容器不支持的组合只返回明确诊断
2. 完整 HTTP 文件下载服务替代现有控制面下载能力
3. 云存储、对象存储和 DVR 检索
4. FLV/PS 文件点播主路径；文件点播首批聚焦 MP4

---

## 与 ABLMediaServer 对比后的主要缺口

| 能力 | ABL 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| 本地 MP4 文件回放 | `NetClientReadLocalMediaFile.*` | 无统一 MP4 文件 VOD | Phase 01/03 |
| MP4/PS/TS/FMP4 录制 | `StreamRecordMP4.*`、`StreamRecordPS.*`、`StreamRecordTS.*`、`StreamRecordFMP4.*` | 无统一录制模型 | Phase 01/02 |
| 多文件回放 | `NetServerReadMultRecordFile.*` | 无 | Phase 03 |
| 文件元数据与生命周期 | `RecordFileSource.*` | HLS 有局部落盘，缺统一索引 | Phase 02 |
| RTSP replay 控制 | `版本信息.txt`、`NetRtspServer*` | 只有 live path | Phase 04 |
| RTMP/HTTP-FLV/WS-FLV 文件回放 | `版本信息.txt` | 只有 live path | Phase 04 |
| seek/pause/scale | `NetClientReadLocalMediaFile.cpp` | 无 | Phase 03/04 |
| ABL 非标准兼容 | `版本信息.txt` | 无 | Phase 05 |
| 真正视频帧率计算 | `版本信息.txt` 多次强调 | 缺统一策略 | Phase 01/05 |
| HTTP-MP4 chunked 回放行为 | `NetServerHTTP_MP4.*`、`版本信息.txt` | 无 | Phase 05 |

---

## ABL 版本信息的关键结论

1. `readMp4FileCount` 是 ABL 本地 MP4 回放的重要控制项，默认播放一次，`-1` 无限循环，正数表示循环次数
2. 本地 MP4 回放支持 `pause`、`resume`、`scale`、`seek`，并要求 seek 越界返回明确错误
3. 8x、16x 回放时应主动丢帧，只发送关键帧，避免高倍速导致的网络和 CPU 放大
4. 回放结束不是简单关闭，而是根据循环配置重新 `seek` 到文件头后继续调度
5. 真实视频帧率需要动态计算，不能固定假设为 25fps；录制封装和 PS 时间戳都要依赖真实帧率
6. 录制和回放不只针对标准理想流，要兼容脏时间戳、厂商 URL、历史控制语义和多文件串联
7. `on_rtsp_replay` 等控制事件包含 `readerCount`、`ip`、`port`、`networkType`、`params`，说明回放需要纳入统一控制面
8. HTTP-MP4 使用 `Transfer-Encoding: chunked`，大块数据要分片发送并限制单次报文大小
9. HLS 回放采用与直播一致的媒体源切片方式，而不是简单拼接历史文件

---

## 参考来源

| 来源 | 路径 |
|------|------|
| ABL 版本功能信息 | `vendor-ref/ABLMediaServer-src-2026-05-09/版本信息.txt` |
| 本地 MP4 回放 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/NetClientReadLocalMediaFile.*` |
| 文件元数据管理 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/RecordFileSource.*` |
| 多文件回放 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/NetServerReadMultRecordFile.*` |
| 录制实现 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/StreamRecord*.{h,cpp}` |
| HTTP-MP4 / HLS / FLV 服务端 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/NetServerHTTP_MP4.*`、`NetServerHLS.*`、`NetServerHTTP_FLV.*` |
| 本项目 codec / 协议基础 | `crates/foundation/cheetah-codec/`、`crates/protocols/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [mp4-vod-record-architecture.md](mp4-vod-record-architecture.md) | 已完成 | 总体架构、crate 边界、数据流、配置和控制面 |
| [mp4-vod-record-abl-gap-analysis.md](mp4-vod-record-abl-gap-analysis.md) | 已完成 | ABL 行为、版本信息结论、本地缺口 |
| [phase-01-codec-container-writers.md](phase-01-codec-container-writers.md) | 已完成 | `cheetah-codec` 的 MP4/FLV/HLS/PS/TS/FMP4 容器能力 |
| [phase-02-record-module-multiformat.md](phase-02-record-module-multiformat.md) | 已完成 | 统一录制模块、文件元数据、事件与控制 API |
| [phase-03-mp4-vod-core-driver.md](phase-03-mp4-vod-core-driver.md) | 已完成 | `cheetah-mp4-core`、`cheetah-mp4-driver-tokio`、`cheetah-mp4-module` |
| [phase-04-cross-protocol-vod-seek.md](phase-04-cross-protocol-vod-seek.md) | 已完成 | RTSP/RTMP/HTTP-FLV/WS-FLV 点播和 seek 接入 |
| [phase-05-compat-interop-fuzz.md](phase-05-compat-interop-fuzz.md) | 已完成 | ABL 兼容、互操作、fixture、属性测试、fuzz |

---

## 渐进式执行顺序

1. **Phase 01** — 先补齐 `cheetah-codec` 的 classic MP4、FLV、PS、HLS、TS、FMP4 录制导出能力
2. **Phase 02** — 建立统一 `record` 模块、文件索引和录制控制 API
3. **Phase 03** — 建立 MP4 VOD 三段式 crate，打通文件 reader、seek、pause、speed、loop 和多文件串联
4. **Phase 04** — 接入 `RTSP/RTMP/HTTP-FLV/WS-FLV` 协议播放和控制
5. **Phase 05** — 补齐 ABL 非标准兼容、真实样例、fuzz 和生产化验证
