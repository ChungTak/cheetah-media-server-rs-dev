# Phase 01 — RTSP 厂商兼容与协议健壮性

- **状态**: 未开始
- **范围**: 宇视/大华/海康摄像头兼容、RTCP RR、OPTIONS 心跳、RTP 时间戳修正
- **完成标准**: 主流 IPC 摄像头可正常拉流，UDP 播放不因缺少 RTCP RR 断流

---

## 1.1 宇视摄像头 SDP 编码探测兼容

**问题**: 宇视摄像头 SDP 可能报告错误的编码类型（如声明 H.264 实际发送 H.265）。

**方案**: 在收到第一个 I 帧时，通过 NALU type 探测实际编码，若与 SDP 不符则重新初始化解包器。

**实现位置**: `cheetah-rtsp-module` publish 管线，在首帧到达时增加编码验证逻辑。

---

## 1.2 大华摄像头 Digest 认证流程兼容

**问题**: 大华摄像头在 OPTIONS 阶段就返回 401 要求 Digest 认证（标准流程是 DESCRIBE 才要求）；trackID 从 0 开始。

**方案**:
- RTSP 客户端（pull job）在 OPTIONS 收到 401 时自动重试带认证
- trackID 解析支持从 0 开始的编号

**实现位置**: `cheetah-rtsp-module` client_pull 认证重试逻辑。

---

## 1.3 海康 NVR trackID/Content-Base 兼容

**问题**:
- 海康 `a=control:` 可能包含完整 RTSP URL（如 `rtsp://admin:pwd@ip:554/trackID=1`）
- 回放模式 RTP payload 可能是 PS 封装
- Content-Base 头需要用于 SETUP URL 构建

**方案**:
- `a=control:` 解析时检测完整 URL 并正确提取 trackID
- 增加 PS 解封装路径（复用 `cheetah-codec` 的 `ps.rs`）
- SETUP URL 构建优先使用 Content-Base

**实现位置**: `cheetah-rtsp-module` SDP 解析 + SETUP URL 构建。

---

## 1.4 RTCP RR 响应

**问题**: 部分摄像头（大华）在 UDP 模式下要求客户端定期发送 RTCP Receiver Report，否则断流。

**方案**: RTSP 播放会话（UDP 模式）定期发送 RTCP RR 包，包含接收统计（丢包率、jitter 等）。

**实现位置**: `cheetah-rtsp-driver-tokio` UDP 传输层，新增 RTCP RR 定时发送。

---

## 1.5 RTSP 客户端 OPTIONS 心跳

**问题**: 长时间拉流时，部分设备/中间件会因无活动超时断开 RTSP 会话。

**方案**: RTSP pull job 客户端每 25 秒发送 OPTIONS 请求作为心跳（支持带 Digest 认证）。

**实现位置**: `cheetah-rtsp-module` client_pull 心跳循环。

**当前状态**: 已有 `heartbeat_mode` 配置（GET_PARAMETER/OPTIONS），需验证 OPTIONS 模式是否完整实现。

---

## 1.6 RTP 时间戳零值修正

**问题**: 部分 GB28181 设备发送的 RTP 包时间戳为 0 或不递增。

**方案**: 检测到连续零时间戳时，使用本地时钟生成递增时间戳。

**实现位置**: `cheetah-codec` 时间戳归一化层，增加零值检测与本地时钟回退。
