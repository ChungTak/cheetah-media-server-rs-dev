# 15 · 发布证据模板

> 每个候选版本复制本文件为 `release_evidence_<version>.md`。没有命令、日志或制品链接的结论不得标记通过。

## 1. 候选版本

| 字段 | 值 |
| --- | --- |
| Cheetah commit | |
| avcodec version | |
| avcodec full revision | |
| Rust toolchain | |
| OS/arch | |
| Candidate artifact | |
| SBOM | |
| License report | |
| 测试时间 | |
| 执行人 | |

## 2. Feature/Profile 制品

| Lane | Features | Build 命令 | Artifact/checksum | 结果 |
| --- | --- | --- | --- | --- |
| C0 | default | | | |
| C1 | audio | | | |
| C2 | image + NativeFree | | | |
| C3 | video + NativeFree | | | |
| C4 | image/video + Software | | | |
| C5 | media-processing-cpu | | | |
| C6 | release | | | |

附：

- C0 `cargo tree` 无 avcodec 证据：
- NativeFree 动态链接检查：
- Software 动态库版本：
- Cheetah 直接依赖只有顶层 avcodec 的检查：
- FDK-AAC 发行许可结论：

## 3. Preflight

| Profile | Operation/codec | Available | Backend/format | Diagnostic |
| --- | --- | --- | --- | --- |
| NativeFree | | | | |
| Software | | | | |

附完整 `/api/v1/processing/preflight` 响应和启动日志，确认 capability operation 与表格一致。

## 4. 功能矩阵

| ID | 场景 | 输入 fixture | 输出验证器 | 结果/Artifact |
| --- | --- | --- | --- | --- |
| IMG | 图片算子/JPEG | | | |
| SNAP | H.264/H.265/MJPEG snapshot | | | |
| AUD | G711/AAC/Opus/MP3 matrix | | | |
| VID | H.264/H.265/MJPEG matrix | | | |
| ABR | 1–4 档梯度 | | | |
| MIX-A | 音频混音 | | | |
| MIX-V | 固定宫格 | | | |
| SUB | CEA/WebVTT | | | |

PNG 输出 Unsupported 证据：

## 5. 协议 E2E

| ID | 流程 | 客户端/版本 | 数据面证据 | 结果 |
| --- | --- | --- | --- | --- |
| E2E-IMG | live → JPEG | | | |
| E2E-FLV | G711/Opus → AAC → RTMP/HTTP-FLV | | | |
| E2E-WEB | AAC/MP3 → Opus → Chrome WHEP | | | |
| E2E-PRX | RTSP pull → derived → consumer | | | |
| E2E-HLS | ABR + CEA → WebVTT | | | |

浏览器证据必须附 getStats、实际 inbound packets/frames/samples 和页面结果；HTTP 2xx 不足以通过。

## 6. 安全与故障

| 场景 | 期望 | 证据 | 结果 |
| --- | --- | --- | --- |
| admission deny | 无配额/租约/任务/幂等成功记录 | | |
| target conflict | 原子回滚 | | |
| 配额耗尽 | ResourceExhausted | | |
| deadline/cancel | bounded cleanup | | |
| corrupt bitstream | 稳定错误、不崩溃 | | |
| worker panic | Job Failed、资源释放 | | |
| source reconnect | 按策略恢复/失败 | | |
| module restart | 无旧 registration 干扰 | | |
| engine shutdown | leak report 为空 | | |
| 非法图片/字体 | 拒绝且无路径泄漏 | | |

## 7. 性能与长稳

| Benchmark | FPS | CPU | RSS | P95 latency | Drop | 基线差异 |
| --- | --- | --- | --- | --- | --- | --- |
| 720p transcode | | | | | | |
| 1080p transcode | | | | | | |
| 3-rung ABR | | | | | | |
| 4-input mosaic | | | | | | |
| 8-input audio mix | | | | | | |

24 小时 soak：

- 开始/结束时间：
- workload：
- source reconnect/module restart 次数：
- RSS/worker/job 曲线：
- terminal leak report：
- artifact：

任一指标相对批准基线回退超过 10% 时，填写批准人和原因；否则发布失败。

## 8. 破坏性迁移

- 旧 FFmpeg Rust API 已删除：
- 旧 Native/ZLM route 已删除：
- 旧 YAML/HTTP 请求拒绝证据：
- `image`/FFmpeg/backend 直接依赖检查：
- SystemArchitecture/AGENTS/README/config 同步：

## 9. 最终签署

| 角色 | 结论 | 姓名/时间 |
| --- | --- | --- |
| 代码负责人 | | |
| 安全/许可 | | |
| 性能/稳定性 | | |
| 发布负责人 | | |

最终结论只能是 `PASS` 或 `BLOCKED`。`BLOCKED` 必须列出 task ID、owner、解除条件和下一次验证命令。
