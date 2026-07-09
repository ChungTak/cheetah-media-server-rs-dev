# RTSP 协议完善设计与开发计划总索引

- **状态**：已完成
- **目标**：参考 ZLMediaKit 等成熟 C++ 流媒体服务器，补齐 RTSP 协议缺口，提升兼容性、安全性和鲁棒性
- **方法**：对比 ZLMediaKit 实现与本地现状，按优先级分阶段补齐功能
- **完成判定**：所有阶段任务完成，通过互操作测试和 fuzz 回归

## 当前仓库事实

- RTSP 已实现 `core + driver-tokio + module` 三段式架构
- 支持 RTSP 1.0 全部标准方法（OPTIONS/DESCRIBE/ANNOUNCE/SETUP/PLAY/PAUSE/RECORD/TEARDOWN/GET_PARAMETER/SET_PARAMETER）
- 支持 RTP over TCP（interleaved）、RTP over UDP（unicast）、RTP multicast、RTSP-over-HTTP tunnel 四种传输
- 支持 Basic + Digest（MD5）认证，服务端和客户端双向
- 支持 H.264/H.265/AV1/VP8/VP9/AAC/Opus/G.711A/G.711U/ADPCM/MP3 编解码
- 支持 RTSP 推流（ANNOUNCE/RECORD）、拉流（DESCRIBE/PLAY）
- 支持后台 Pull/Push/Relay 任务，含重试退避
- 拥有 17 个集成测试、9 个属性测试、18 个 fuzz target
- MPEG-PS/MP2P 兼容探测已实现

## 与 ZLMediaKit 对比后的主要缺口

| 能力 | ZLMediaKit 参考点 | 本地状态 | 计划处理 |
|------|-------------------|----------|----------|
| RTSPS（TLS） | `RtspSessionWithSSL` 模板包装 | ❌ 未实现 | Phase-01 |
| SHA-256 Digest 认证 | 标准 RFC 7616 | ❌ 仅 MD5 | Phase-01 |
| 静音音频生成 | `MuteAudioMaker` 视频驱动 AAC 静音帧 | ❌ 未实现 | Phase-02 |
| RTP 重排序缓冲区 | `PacketSortor` 含 seq 回绕检测 | 配置存在但实现不完整 | Phase-02 |
| RTCP-FB（NACK/PLI/FIR） | 通过 RTCP 扩展支持 | ❌ 未实现 | Phase-02 |
| 非标兼容（厂商 quirks） | `.sdp` 后缀剥离、心跳兼容、seq 重置检测 | 部分实现 | Phase-03 |
| 断连续推（continue push） | 可配置延迟保持源存活 | ❌ 未实现 | Phase-03 |
| 直接代理模式（Direct Proxy） | 零解码 RTP 转发 | ❌ 未实现 | Phase-04 |
| Scale/Speed 控制 | 客户端 seek + speed | 仅解析，未执行 | Phase-04 |
| RTCP-XR 扩展报告 | 基础支持 | ❌ 未实现 | Phase-05 |
| FEC（前向纠错） | 未实现 | ❌ 未实现 | Phase-05（评估） |

## 总体约束

- 严格遵守 `core + driver + module` 三段式，Sans-I/O 硬约束不可破坏
- 新增功能必须补测试（单元 + 属性 + fuzz）
- 兼容性逻辑集中管理，不散落在热路径
- 热路径禁止阻塞，所有缓冲区必须有上界
- TLS 实现放在 driver 层，core 不感知
- 静音音频生成放在 `cheetah-codec` 或 module 层，不放在 core
- 编解码器扩展统一通过 `cheetah-codec` 管理

## 参考来源

- `vendor-ref/ZLMediaKit/src/Rtsp/` — RTSP 服务器/客户端/推流/组播完整实现
- `vendor-ref/ZLMediaKit/src/Common/MediaSink.h` — MuteAudioMaker 静音音频
- `vendor-ref/ZLMediaKit/src/Rtcp/` — RTCP SR/RR/SDES/BYE/FCI
- RFC 2326 — RTSP 1.0
- RFC 7826 — RTSP 2.0（参考，不实现）
- RFC 7616 — HTTP Digest Access Authentication（SHA-256）
- RFC 4585 — Extended RTP Profile for RTCP-Based Feedback
- RFC 5109 — RTP Payload Format for Generic FEC
- RFC 8866 — SDP

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| `rtsp-architecture.md` | 规划中 | 整体架构演进与模块边界 |
| `rtsp-phase-01-security-tls-auth.md` | 规划中 | RTSPS + SHA-256 Digest |
| `rtsp-phase-02-media-pipeline-enhance.md` | 规划中 | 静音音频 + RTP 重排序 + RTCP-FB |
| `rtsp-phase-03-compat-robustness.md` | 规划中 | 非标兼容 + 断连续推 + 厂商 quirks |
| `rtsp-phase-04-proxy-performance.md` | 规划中 | 直接代理 + Scale/Speed + 性能优化 |
| `rtsp-phase-05-extended-features.md` | 规划中 | RTCP-XR + FEC 评估 + 未来扩展 |

## 任务状态总表

| 阶段 | 任务 | 状态 | 计划文件 |
|------|------|------|----------|
| 1.1 | driver-tokio 添加 TLS acceptor（rustls） | ✅ 完成 | phase-01 |
| 1.2 | 客户端支持 `rtsps://` 连接 | ✅ 完成 | phase-01 |
| 1.3 | 配置模型添加 TLS 证书路径 | ✅ 完成 | phase-01 |
| 1.4 | Digest 认证支持 SHA-256 算法 | ✅ 完成 | phase-01 |
| 1.5 | 认证 nonce 防重放增强 | ✅ 完成 | phase-01 |
| 2.1 | 静音音频生成器（AAC silence） | ✅ 完成 | phase-02 |
| 2.2 | RTP 重排序缓冲区状态机 | ✅ 已有 | phase-02 |
| 2.3 | RTCP-FB NACK 发送/接收 | ✅ 完成 | phase-02 |
| 2.4 | RTCP-FB PLI/FIR 请求关键帧 | ✅ 完成 | phase-02 |
| 2.5 | RTP seq 回绕与重置检测 | ✅ 完成 | phase-02 |
| 3.1 | SDP 后缀剥离（EasyDarwin 兼容） | ✅ 完成 | phase-03 |
| 3.2 | 心跳模式兼容（RTCP/GET_PARAMETER 交替） | ✅ 完成 | phase-03 |
| 3.3 | 断连续推（configurable source keep-alive） | ✅ 完成 | phase-03 |
| 3.4 | Transport 协商容错（461 降级重试） | ✅ 完成 | phase-03 |
| 3.5 | Control URL 格式兼容（绝对/相对） | ✅ 完成 | phase-03 |
| 3.6 | 缺失采样率默认值填充 | ✅ 完成 | phase-03 |
| 4.1 | Direct Proxy 零解码 RTP 转发 | ✅ 完成 | phase-04 |
| 4.2 | Scale/Speed 头处理 | ✅ 已有 | phase-04 |
| 4.3 | UDP NAT 穿透增强 | ✅ 完成 | phase-04 |
| 4.4 | 端口池随机化分配 | ✅ 完成 | phase-04 |
| 5.1 | RTCP-XR 基础支持 | ✅ 完成 | phase-05 |
| 5.2 | FEC 可行性评估 | 📋 评估完成(不实现) | phase-05 |
| 5.3 | RTSP REDIRECT 支持 | ✅ 完成 | phase-05 |

## 渐进式执行顺序

1. **Phase-01（安全层）**：RTSPS 和 SHA-256 认证是安全基础，优先实现。TLS 在 driver 层添加，不影响 core。
2. **Phase-02（媒体管线增强）**：静音音频和 RTP 重排序直接影响播放质量，RTCP-FB 是关键帧请求的基础。
3. **Phase-03（兼容性）**：解决真实设备互操作问题，提升生产环境可用性。
4. **Phase-04（代理与性能）**：Direct Proxy 模式大幅降低 CPU 开销，适合大规模转发场景。
5. **Phase-05（扩展特性）**：RTCP-XR 和 FEC 属于锦上添花，优先级最低。

## 阶段完成后的统一检查

```bash
cargo fmt
cargo clippy -p cheetah-rtsp-core
cargo clippy -p cheetah-rtsp-driver-tokio
cargo clippy -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-core
cargo test -p cheetah-rtsp-driver-tokio
cargo test -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-property-tests
```
