# Phase 02: RTMP 入口归一化与秒开对齐

- 状态：已完成（任务 1-4 已完成）
- 范围：收敛 RTMP 发布入口的时间戳与参数集行为，确保进入引擎前统一为稳定的 `AVFrame + TrackInfo`。
- 完成标准：RTMP 同协议推拉稳定，且为跨协议桥接提供一致时间轴基础。

## 具体任务

### 1. RTMP 入口时间戳归一化接入（已完成）

- [x] 盘点当前 RTMP 发布入口对 PTS/DTS 的来源与回退逻辑。
- [x] 接入统一单调策略（优先复用 `cheetah-codec` 能力），避免 module 私有分叉。
- [x] 明确音频与视频各自的时间戳优先级与缺失补偿策略。

### 2. GOP 秒开数据准备一致性（已完成）

- [x] 对齐关键帧与参数集补发策略，确保新订阅者首帧可解码。
- [x] 验证 H264/H265/VP8/VP9/AV1 视频轨路径一致性。
- [x] 验证 AAC/Opus/G711/MP3 音频轨时间轴连续性。

### 3. RTMP 主线回归（已完成）

- [x] 同协议 `RTMP -> RTMP` 推拉流回归。
- [x] 长时播放（>= 10 分钟）检查时间戳漂移与卡顿。
- [x] 压测下检查队列上界与慢订阅者隔离。

### 4. 变更收口（已完成）

- [x] 补充单元测试与集成测试。
- [x] 记录已知兼容性差异与后续 bridge 风险清单。

## 已完成说明（2026-04-28）

- 视频入口：维持 `DTS=RTMP tag timestamp`、`PTS=DTS+CTS` 优先级；引入 `cheetah-codec::WrapUnwrapper` 处理 32-bit 回绕，并对回退时间戳做严格单调补偿。
- 音频入口：维持 `PTS=DTS=RTMP tag timestamp`，同样引入回绕处理和单调补偿。
- 异常处理：时间戳回退被修复时标记 `FrameFlags::DISCONTINUITY`，继续推流而非中断会话。
- 回归：新增 RTMP 入口音视频回退时间戳单测，验证单调性与视频 CTS 偏移保持。
- GOP 秒开一致性：H265 在 `hvcc` 缺失但存在 `vps/sps/pps` 时，回退构建最小 `hvcc` 并纳入 RTMP bootstrap 配置发送，避免新订阅者首帧解码依赖源端额外配置包；补充 VP8/VP9/AV1 bootstrap 配置路径一致性测试。
- 音频连续性：补充 AAC/Opus/G711/MP3 统一回退时间戳修复回归，验证 `PTS=DTS`、单调推进与 `DISCONTINUITY` 标记语义。
- 任务 4 收口：补充 H265 缺失配置下的 bootstrap 安全跳过单测，并新增 `tests/rtmp_module_push_job_resilience.rs` 集成回归，覆盖 push 源流不存在时的错误路径（模块保持运行且可平滑停止）；同时将播放/推流 bootstrap 与媒体发送错误从静默吞错改为显式日志与受控退出。
- 验证结果：`cargo test -p cheetah-rtmp-module` 当前通过 `57` 个单元测试与 `1` 个集成测试，未出现失败或回归。

## 已知兼容性差异与 Bridge 风险清单（2026-04-28）

- 差异 1：RTMP 推流端在上游时间戳回退时采用“单调修复并打 `DISCONTINUITY`”策略，不保留源端原始倒退时间轴；Bridge 侧若需要对齐外部绝对时间，必须以 `DISCONTINUITY` 作为切段边界。
- 差异 2：H265 在 `hvcc` 缺失但 `vps/sps/pps` 存在时会回退构建最小配置；若三者同时缺失则仅跳过序列头发送，依赖后续关键帧携带可解码参数集。
- 差异 3：push 作业在源流不存在时按重试退避持续重试，不主动将 module 标记为失败；运维侧需通过告警日志识别“配置存在但源未上线”状态。
- 风险 1（Phase 03）：RTSP->RTMP 桥接若直接复用 RTSP 抖动后时间轴，可能与 RTMP 单调修复叠加导致片段时长突变，需在 bridge 明确“源时间轴/传输时间轴”边界。
- 风险 2（Phase 03）：RTMP->RTSP 桥接时，`DISCONTINUITY` 到 RTP 时间戳/序列号重置策略尚未统一，若处理不一致可能触发播放器卡顿或等待关键帧超时。
- 风险 3（Phase 03）：多编码（H264/H265/VP9/AV1）跨协议切换时参数集补发时机尚未矩阵化验证，需在双向桥接阶段补齐 codec*transport 组合回归。

## 下一步

1. 进入 Phase 03，推进 RTSP<->RTMP 双向桥接时间戳与缓冲策略统一。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-rtmp-module`
- `cargo test -p cheetah-rtmp-module`
- 必要时增加 `cheetah-codec` 相关测试回归
