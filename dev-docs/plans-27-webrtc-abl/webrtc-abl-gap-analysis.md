# WebRTC ABL 差距分析

- **状态**: 已完成
- **范围**: 设计文档

## 已有能力

本地 WebRTC 已经具备以下基础，后续应复用而不是重写：

- `core + driver + module` 三段式 crate。
- WHEP/WHIP 相关 HTTP 路由。
- ZLM/SMS URL 兼容解析、ICE restart/trickle candidate 解析、base64 与 msid rewrite 辅助。
- codec policy，已覆盖 H264/H265/VP8/VP9/AV1 与 Opus/G711/AAC 的偏好表达。
- RTP/RTCP、BWE、simulcast、DataChannel、P2P、fuzz/property-test 的已有规划和部分实现。
- module 配置中已有 ICE-lite、UDP/TCP、public IP、candidate hostname、队列、timeout、BWE、RTX/reorder、bootstrap、B-frame filter 等字段。

## 主要缺口

| 缺口 | ABL 依据 | 本地风险 | 优先级 |
| --- | --- | --- | --- |
| OPTIONS 快速关闭与 CORS 私网头 | `ResponseOPTIONS` 与版本记录中多路快速播放修复 | 浏览器预检或私网访问失败 | P0 |
| ABL 风格 `/rtc/v1/whep` URL | ABL POST/Location 与 stream arrive URL | 历史客户端无法复用同一 URL | P0 |
| 从 offer 动态提取视频/Opus payload | 2025-06-12、2025-12-01 发布记录 | 使用固定 payload 导致浏览器不解码 | P0 |
| HTTP 连接关闭不等于播放结束 | 2025-06-13 发布记录 | 会话过早销毁，播放立即断开 | P0 |
| G711 直通、AAC/MP3 转 Opus 策略 | 2025-07-28、2025-11-26、2025-12-01 发布记录 | 音频不出声或时间戳漂移 | P1 |
| 回放帧号映射 RTP timestamp | 2025-12-25 发布记录 | 回放音画不同步 | P1 |
| IDR 前参数集补发 | 2025-10-14 发布记录 | 首屏黑屏或切流后解码失败 | P1 |
| WebRTC 播放 URL 与对象查询 | 2025-12-26、2025-12-29 发布记录 | 控制面无法定位播放对象 | P2 |
| 播放断开事件 | 2026-02-02 发布记录 | 业务侧无法感知播放结束 | P2 |
| UDP 发送端口范围配置 | 2025-08-08 发布记录 | 防火墙/NAT 部署不稳定 | P2 |

## 非标准兼容清单

后续实现应显式命名为 ABL compat 或 vendor compat，避免隐藏在主路径条件分支中：

- 接受 `/rtc/v1/whep/?app=<app>&stream=<stream>`，并与现有 `/whep` 入口统一到同一会话创建逻辑。
- OPTIONS 返回 `200`、`Content-Length: 0`、CORS 允许源，并在配置开启时返回 `Access-Control-Request-Private-Network: true` 兼容头。
- POST 响应 `201 Created`、`Content-Type: application/sdp`、`Location` 指向可 PATCH/DELETE 的会话资源或兼容 URL。
- offer 中缺失目标视频 payload 时，返回清晰错误并释放半初始化会话。
- 对 AAC/MP3 输入优先输出浏览器可解的 Opus；当 offer 接受 G711 时，G711A/G711U 直接透传。
- 对回放场景使用帧号或源时间戳生成稳定 RTP timestamp，不用 live 自增逻辑覆盖。

## 风险与约束

- ABL 代码中存在固定 fingerprint、固定 payload、固定 URL 拼接和对象内聚度过高的问题，只能提取行为，不应迁移结构。
- 音频转码会引入 CPU 与依赖风险；实现前需要确认 `cheetah-codec` 当前可用的音频编解码能力，缺失时先以策略和接口落地。
- 会话断开事件涉及 SDK/控制面边界，第一阶段应先复用现有 `SystemEvent::Stream` 或 module 指标，避免新增独立 hook 子系统。
- UDP 端口范围属于 driver 资源分配，不能在 module 直接绑定 socket。

## 完成定义

本计划完成时应达到：

- 浏览器 WHEP 播放、ABL 风格 WHEP URL、CORS/OPTIONS 预检均可稳定工作。
- payload、音频、时间戳和参数集行为有单元测试或互操作样例覆盖。
- HTTP 会话和 WebRTC 播放会话生命周期分离，播放结束能观测。
- 所有 vendor quirks 集中在 compat 层或命名清晰的配置开关中。
