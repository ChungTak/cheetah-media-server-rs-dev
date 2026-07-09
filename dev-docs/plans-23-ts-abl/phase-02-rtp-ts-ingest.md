# Phase 02 — RTP-TS 输入与国标兼容

- **状态**: 计划中
- **范围**: 新增或完善 RTP-TS ingest 能力，覆盖 ABL 版本信息中反复强调的 RTP 接收缓冲区切割和海康下级平台国标流兼容。
- **完成标准**: UDP/TCP RTP payload 中的 TS 能被按 SSRC 分流、demux、发布为 engine stream，并能容忍真实设备常见非理想输入。

---

## 2.1 RTP packet parser

**ABL 参考**: `NetServerRecvRtpTS_PS.cpp` 直接按 `_rtp_header` 读取 version、ssrc，并用 payload 头判断 PS/TS。

**本地要求**:

1. RTP parser 支持 version、padding、extension、CSRC count、marker、payload type、sequence、timestamp、ssrc。
2. header 长度不能固定为 12；必须跳过 CSRC 和 extension。
3. padding bit 置位时从 payload 尾部扣除 padding bytes。
4. 非 RTP v2、header 越界、payload 为空输出 diagnostic 并丢包。
5. 增加字节序测试，覆盖 Linux/ARM 等平台。

---

## 2.2 SSRC session router

**ABL 参考**: `rtpClient = BaseRecvRtpSSRCNumber + rtpHeadPtr->ssrc`，每个 SSRC 对应一个输入对象。

**本地要求**:

1. 每个 SSRC 绑定一个 bounded ingest session。
2. 同一 SSRC 来源地址变化时输出 diagnostic；是否接纳由配置控制。
3. `rtp_ts.max_sessions` 限制 session 数。
4. `rtp_ts.session_idle_timeout_ms` 到期清理 session 并释放 publisher lease。
5. sequence gap、timestamp rollback、marker 缺失只诊断，不立即断流。

---

## 2.3 TS / PS 探测

**ABL 参考**: payload 开头匹配 PS 头则创建 PS input，否则创建 TS input。

**本地要求**:

1. TS 探测优先查找 `0x47` 且后续 188 间隔仍为 `0x47`。
2. PS 探测识别 `0x000001BA`，本轮不实现 PS demux 时输出明确 `UnsupportedRtpPayload::Ps`。
3. 如果 payload 既不像 TS 也不像 PS，允许短暂等待下一包；超过阈值后关闭 session。
4. 支持 RTP payload 前有少量厂商私有前缀时重同步到 TS sync byte。

---

## 2.4 RTP-TS payload 切割

**ABL 原始行为**: `RtpTSStreamInput::InputNetData()` 要求 `(nDataLength - 12) % 188 == 0`。

**本地增强**:

1. 对 188 对齐 payload 走快路径，逐包喂 demux。
2. 对非 188 对齐 payload 走兼容路径，整体喂 `MpegTsDemuxer::push()`，依赖重同步和 remainder。
3. 支持一个 TS packet 跨 RTP 包的场景，但跨包 remainder 必须有上限。
4. 支持一个 RTP payload 包含多个 TS packet。
5. 连续 sync loss 超过阈值时重置 demuxer 状态。

---

## 2.5 发布到 engine

**本地要求**:

1. 每个 RTP-TS session acquire 一个 publisher lease。
2. `TrackFound` 累积并 `update_tracks()`，新轨追加，不覆盖旧轨。
3. `Frame` 使用 demux 输出的 track_id、codec、pts/dts、keyframe。
4. `FrameRateEstimator` 输出有效 fps 后更新 stream metadata。
5. cancel、idle timeout、fatal error 时 flush demuxer 并 release lease。

---

## 测试要求

1. 单 SSRC H264/AAC RTP-TS 发布。
2. 两个 SSRC 同端口分流到两个 stream。
3. RTP header extension 和 CSRC 输入。
4. payload 非 188 对齐但可重同步。
5. sequence gap 和 timestamp rollback 诊断。
6. PS payload 返回明确 unsupported。
7. idle timeout 释放 publisher lease。

---

## 验证命令

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo clippy -p cheetah-ts-driver-tokio
cargo clippy -p cheetah-ts-module
cargo test -p cheetah-codec ts_
cargo test -p cheetah-ts-driver-tokio rtp_ts
cargo test -p cheetah-ts-module rtp_ts
```
