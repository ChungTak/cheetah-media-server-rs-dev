# Phase 03: RTSP <-> RTMP 双向桥接稳定性

- 状态：已完成（任务 1-4 已完成）
- 范围：修复跨协议桥接链路（`RTSP->RTMP` 与 `RTMP->RTSP`）中的 `DTS out of order`、首开卡住与播放停滞问题。
- 完成标准：双向桥接在 TCP/UDP 传输模式下均稳定连续，无 `DTS out of order` 引发的冻结。

## 具体任务

### 1. RTSP -> RTMP 桥接修正（已完成）

- [x] 追踪 `AVFrame` 从 RTSP publish 到 RTMP mux 的时间戳映射链路。
- [x] 在桥接边界统一时间轴与单调约束，消除 FLV `DTS out of order`。
- [x] 增加最小复现回归（固定输入样本 + 拉流断言）。

### 2. RTMP -> RTSP 桥接修正（已完成）

- [x] 追踪 RTMP ingest 到 RTSP packetize 的时间戳映射链路。
- [x] 校验 RTP timestamp 生成策略与轨道类型匹配（视频/音频分别验证）。
- [x] 覆盖 RTSP TCP interleaved 与 UDP unicast 两种播放模式。

### 3. 多编码一致性覆盖（已完成）

- [x] 视频：H264/H265/AV1/VP8/VP9
- [x] 音频：AAC/Opus/G711/MP3
- [x] 校验跨协议时首帧可解码、播放连续、无明显 A/V 漂移

### 4. 双向桥接回归矩阵（已完成）

- [x] `RTSP(TCP)->RTMP`、`RTSP(UDP)->RTMP`
- [x] `RTMP->RTSP(TCP)`、`RTMP->RTSP(UDP)`
- [x] 长时回归（>= 30 分钟）与重复起播回归（连续多次拉流）

## 下一步

1. 进入 Phase 04，补齐全矩阵自动化回归脚本与可观测基线。
2. 将跨协议桥接关键告警/耗时指标纳入运维排障清单。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-rtsp-module`
- `cargo clippy -p cheetah-rtmp-module`
- `cargo test -p cheetah-rtsp-module`
- `cargo test -p cheetah-rtmp-module`
