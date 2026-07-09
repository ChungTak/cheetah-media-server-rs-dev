# RTMP Phase 04 — 协议兼容性与鲁棒性

- 状态：已完成
- 范围：处理非标准 RTMP 行为、厂商 quirks、Aggregate 消息、复杂握手、带宽检测
- 完成标准：OBS/FFmpeg/VLC/SRS/Wirecast 等主流客户端推拉流无兼容性问题，异常输入不导致 panic 或资源泄漏

## 目标

提高 RTMP 实现的互操作性和鲁棒性：
1. 正确处理主流客户端的非标准行为。
2. 实现 Aggregate 消息拆分。
3. 可选支持复杂握手（HMAC-SHA256）。
4. 基础带宽检测能力。
5. 建立厂商 quirks 集中管理机制。

## 设计原则

- **入口宽容**：接受非标准输入，不因格式偏差断开连接。
- **内部规范**：所有数据经过规范化后进入引擎。
- **出口稳定**：输出严格符合规范，可预测。
- **集中管理**：所有兼容性处理在 `compat` 模块中显式注册。

## 任务分解

### 4.1 FCPublish/FCUnpublish 处理

**背景**：OBS 在 publish 前发送 `FCPublish`，断开前发送 `FCUnpublish`。FFmpeg 发送 `releaseStream`。这些命令在 RTMP 规范中未明确定义，但主流客户端广泛使用。

**参考**：simple-media-server 将 `FCPublish` 等同于 publish 预通知，`releaseStream` 作为 no-op 响应。

**cheetah-rtmp-core 改动**：

1. **FCPublish**：
   - 解析命令参数（stream name）。
   - 生成 `Output::SendFCPublishResult` 响应（`_result` + `onFCPublish` status）。
   - 不创建发布会话，仅作为预通知记录。
   - 响应格式：
     ```
     _result(txId, null)
     onFCPublish(0, null, {code: "NetStream.Publish.Start", description: "..."})
     ```

2. **FCUnpublish**：
   - 解析命令参数。
   - 生成 `Output::SendFCUnpublishResult` 响应。
   - 触发流结束清理（等同于 deleteStream 语义）。

3. **releaseStream**：
   - 解析命令参数。
   - 生成 `Output::SendReleaseStreamResult`（`_result(txId, null)`）。
   - 不执行任何业务逻辑（no-op）。

**测试**：
- 单元测试：FCPublish/FCUnpublish/releaseStream 命令解析和响应生成。
- 集成测试：OBS 推流完整流程（connect → FCPublish → createStream → publish → ... → FCUnpublish → deleteStream）。

### 4.2 releaseStream 兼容

**背景**：FFmpeg 在 publish 前发送 `releaseStream` 和 `FCPublish`。部分客户端还发送 `FCSubscribe`。

**实现**：
- `releaseStream`：响应 `_result`，no-op。
- `FCSubscribe`：响应 `_result` + `onFCSubscribe` status，no-op。
- 所有未识别的 invoke 命令：记录 warn 日志，响应 `_error` 但不断开连接。

**cheetah-rtmp-core 改动**：
- 在命令分发中增加这些命令的匹配分支。
- 统一的"未知命令"处理路径：`Output::SendCommandError { txn_id, code: "NetStream.Failed" }`。

### 4.3 Aggregate 消息拆分

**背景**：RTMP Aggregate Message (type 22) 将多个子消息打包在一个 chunk message 中。部分编码器使用此格式减少 chunk 开销。

**参考**：simple-media-server 定义了类型但未实现处理（记录日志）。本实现选择完整支持以提高兼容性。

**RTMP Aggregate 格式**：
```
[SubMessage1][BackPointer1][SubMessage2][BackPointer2]...

SubMessage:
  - MessageType (1 byte)
  - PayloadLength (3 bytes, big-endian)
  - Timestamp (4 bytes, 3 bytes + 1 byte extended)
  - StreamID (3 bytes, always 0)
  - Payload (PayloadLength bytes)

BackPointer:
  - PreviousTagSize (4 bytes, big-endian) = 11 + PayloadLength
```

**cheetah-rtmp-core 改动**：
- 新增 `aggregate` 模块，实现 `split_aggregate_message(payload: &[u8]) -> Vec<SubMessage>`。
- 在消息分发层，收到 type 22 时调用拆分，将子消息逐个注入状态机处理。
- 子消息的时间戳 = Aggregate 消息时间戳 + 子消息内部时间戳偏移。
- 错误处理：格式异常时记录日志并丢弃整个 Aggregate 消息，不断开连接。

**测试**：
- 单元测试：Aggregate 消息拆分（正常、空、格式异常）。
- 属性测试：随机子消息组合 → 打包 → 拆分 → 验证一致性。
- Fuzz：Aggregate 消息解析 fuzz target。

### 4.4 复杂握手支持

**背景**：Flash Player 时代的 RTMP 客户端使用 HMAC-SHA256 digest 握手验证服务端身份。现代客户端（OBS/FFmpeg）不使用此机制，但部分老旧设备可能需要。

**参考**：simple-media-server 未实现复杂握手。本实现作为可选 feature 提供。

**设计**：
- 作为 `cheetah-rtmp-core` 的可选 feature：`complex-handshake`。
- 默认关闭，不增加编译时间和二进制大小。
- 实现 HMAC-SHA256 digest 验证（FP9 handshake scheme）。

**握手检测逻辑**：
1. 收到 C1 后，尝试在 offset 位置验证 HMAC-SHA256 digest。
2. 如果验证成功 → 使用复杂握手响应。
3. 如果验证失败 → 回退到简单握手（当前行为）。

**cheetah-rtmp-core 改动**（feature-gated）：
- 新增 `handshake::complex` 模块。
- `HandshakeState` 增加 `ComplexS1` 生成路径。
- 依赖 `hmac` + `sha2` crate（仅在 feature 启用时）。

**测试**：
- 单元测试：HMAC digest 计算和验证。
- 集成测试：使用支持复杂握手的客户端库验证。

### 4.5 厂商 Quirks 集中管理

**目标**：建立统一的厂商特征检测和兼容性处理框架。

**设计**：

```rust
/// 厂商/客户端特征标识
pub enum ClientQuirk {
    /// OBS: 发送 FCPublish，chunk size 通常为 4096
    Obs,
    /// FFmpeg: 发送 releaseStream + FCPublish
    Ffmpeg,
    /// Flash Player: 可能使用复杂握手
    FlashPlayer,
    /// SRS: 特定的 connect 参数格式
    Srs,
    /// 未知客户端
    Unknown,
}

/// Quirks 检测器
pub struct QuirksDetector;

impl QuirksDetector {
    /// 从 connect 命令的参数中检测客户端类型
    pub fn detect_from_connect(args: &ConnectArgs) -> ClientQuirk {
        // 基于 flashVer / swfUrl / tcUrl 等字段判断
    }
}
```

**检测依据**：
- `flashVer` 字段：`FMLE/3.0` (Flash), `LNX 9,0,124,2` (FFmpeg), `OBS ...` (OBS)
- `swfUrl` 字段：OBS 通常不设置
- `tcUrl` 格式差异
- 命令序列模式（FCPublish 出现顺序）

**应用场景**：
- 检测到 OBS → 预期 FCPublish 命令序列。
- 检测到 FFmpeg → 预期 releaseStream 命令。
- 检测到 Flash Player → 尝试复杂握手。
- 未知客户端 → 使用最宽容的处理模式。

**放置位置**：`cheetah-rtmp-core` 的 `compat` 模块。

### 4.6 带宽检测基础

**背景**：部分客户端在连接后请求带宽检测（`_checkbw` / `onBWDone`）。服务端需要正确响应以避免客户端超时。

**实现**：
- 识别 `_checkbw` 命令 → 响应 `_result`。
- 主动发送 `onBWDone(0)` 通知客户端带宽检测完成。
- 不实现实际带宽测量（复杂且收益低），仅做协议层兼容。

**cheetah-rtmp-core 改动**：
- 命令分发增加 `_checkbw` 匹配。
- connect 成功后的响应序列中增加 `onBWDone`。

**测试**：
- 单元测试：`_checkbw` 命令处理。
- 集成测试：验证发送 `_checkbw` 的客户端不会超时。

## 额外鲁棒性改进

### 非标准 Chunk Size 容忍

- 接受任意 chunk size（1 ~ 16MB），不因超大 chunk size 断开。
- 参考 simple-media-server 客户端设置 5,000,000。
- 配置上限 `max_chunk_size`（默认 10,000,000），超过上限记录 warn 但仍接受。

### 时间戳回绕处理

- 当时间戳从大值突然跳到小值时，检测为回绕而非错误。
- 回绕阈值：差值 > 0x7FFFFFFF 视为回绕。
- 回绕后重置时间戳基准，保持单调递增输出。

### 消息大小限制

- 单条消息最大大小限制（默认 16MB）。
- 超过限制时丢弃消息并记录 warn，不断开连接。
- 防止恶意客户端通过超大消息耗尽内存。

### 连接超时

- 握手超时：5 秒（可配置）。
- connect 命令超时：10 秒（握手完成后等待 connect）。
- 空闲超时：60 秒（无数据传输）。
- 所有超时可通过配置调整。

## 测试计划

1. **兼容性矩阵测试**：

| 客户端 | 推流 | 拉流 | 特殊行为 |
|--------|------|------|---------|
| OBS 30+ | ✅ | ✅ | FCPublish, Enhanced RTMP |
| FFmpeg 6+ | ✅ | ✅ | releaseStream, Enhanced RTMP |
| VLC 3+ | ❌ | ✅ | 标准 play |
| FFplay | ❌ | ✅ | 标准 play |
| SRS 5+ | ✅ | ✅ | 互推互拉 |
| simple-media-server | ✅ | ✅ | 互推互拉 |

2. **异常输入测试**：
   - 超大 chunk size 设置。
   - 时间戳回绕。
   - 格式异常的 Aggregate 消息。
   - 未知命令洪泛。
   - 半关闭连接。

3. **Fuzz 扩展**：
   - Aggregate 消息解析 fuzz target。
   - 命令分发 fuzz target（随机 AMF 命令）。
