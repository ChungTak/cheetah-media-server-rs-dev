# Phase-03: 兼容性与鲁棒性 — 非标处理 + 断连续推 + 厂商 Quirks

## 目标

提升与真实设备（IP 摄像头、编码器、各厂商客户端）的互操作性，处理非标准实现，增强生产环境鲁棒性。

## 设计原则

- 入口允许兼容脏数据，内部必须规范化，出口必须稳定可预测
- 兼容逻辑集中在 `core/src/compat/` 模块，显式命名
- 每个 quirk 有独立开关，可通过配置启用/禁用
- 所有兼容处理必须补回归测试

## 任务清单

### 3.1 SDP 后缀剥离（EasyDarwin 兼容）

**范围**：`cheetah-rtsp-core` compat 模块

**问题**：EasyDarwin 等服务器在 ANNOUNCE URL 末尾添加 `.sdp` 后缀

**实现**：
```rust
// core/src/compat/url_quirks.rs
pub fn normalize_rtsp_url(url: &str) -> &str {
    url.strip_suffix(".sdp").unwrap_or(url)
}
```

- 在 ANNOUNCE/DESCRIBE 处理前统一调用
- 配置 `compat.strip_sdp_suffix: true`（默认开启）

### 3.2 心跳模式兼容

**范围**：`cheetah-rtsp-core` + `cheetah-rtsp-driver-tokio`

**问题**：不同客户端/服务器对 keep-alive 方式要求不同：
- 部分设备只认 RTCP SR/RR 作为心跳
- 部分设备只认 GET_PARAMETER/OPTIONS 信令心跳
- 部分设备需要两者交替

**参考**：ZLMediaKit `kRtspBeatType`（issue #642 修复）

**实现**：

1. 配置心跳模式：
   ```yaml
   compat:
     heartbeat_mode: auto  # auto | rtcp | get_parameter | both
   ```

2. `auto` 模式逻辑：
   - 默认发送 RTCP SR/RR
   - 如果对端在 2 个心跳周期内无响应，切换为 GET_PARAMETER
   - 如果仍无响应，尝试 OPTIONS
   - 记录最终生效的模式，后续固定使用

3. driver 层实现心跳 timer：
   - 服务端：session timeout 的 1/3 间隔发送心跳
   - 客户端：根据服务端 `Session: timeout=` 头计算间隔

### 3.3 断连续推（Continue Push）

**范围**：`cheetah-rtsp-module`

**参考**：ZLMediaKit `continue_push_ms` 配置

**问题**：推流端网络抖动断开后快速重连，如果立即释放源会导致所有订阅者断流

**实现**：

1. 发布者断开时不立即释放 StreamKey ownership：
   - 启动 `continue_push_timer`（可配置，默认 10s）
   - 在此期间，源保持存活，订阅者不断开
   - 新的 ANNOUNCE 请求匹配同一 StreamKey 时，接管 ownership

2. 超时后：
   - 释放 ownership
   - 通知所有订阅者源已断开
   - 清理会话状态

3. 配置：
   ```yaml
   compat:
     continue_push_ms: 10000    # 0 = 禁用
   ```

4. 约束：
   - 续推期间不接受不同编解码器配置的新推流（SDP 必须兼容）
   - 续推成功后，seq/timestamp 可能不连续，需要通知订阅者

### 3.4 Transport 协商容错

**范围**：`cheetah-rtsp-core` + `cheetah-rtsp-module`

**问题**：
- 客户端请求 UDP 但服务器只支持 TCP（或反之）
- 客户端 Transport 头格式非标

**实现**：

1. 服务端 Transport 协商降级：
   - 配置 `forced_transport: Option<RtpTransport>`
   - 如果配置了强制传输方式且客户端请求不匹配，返回 461
   - 461 响应包含 `Transport` 头提示可用传输方式
   - 客户端收到 461 后应重新 SETUP

2. 客户端 Transport 降级重试：
   - 配置 `transport_preference: [tcp, udp, http_tunnel]`
   - 收到 461 后自动尝试下一个传输方式
   - 所有方式都失败后报错

3. Transport 头非标解析（`core/src/compat/transport_quirks.rs`）：
   - 容忍多余空格
   - 容忍大小写不一致（`RTP/AVP/TCP` vs `rtp/avp/tcp`）
   - 容忍缺失 `unicast`/`multicast` 标记（默认 unicast）

### 3.5 Control URL 格式兼容

**范围**：`cheetah-rtsp-core` compat 模块

**问题**：SDP 中 `a=control:` 行格式多样：
- 绝对 URL：`rtsp://server/path/trackID=1`
- 相对路径：`trackID=1`
- 带斜杠相对：`/trackID=1`
- 星号：`*`（aggregate control）

**实现**：
```rust
// core/src/compat/url_quirks.rs
pub fn resolve_control_url(base_url: &str, control: &str) -> String {
    if control == "*" {
        return base_url.to_string();
    }
    if control.starts_with("rtsp://") || control.starts_with("rtsps://") {
        return control.to_string();
    }
    // 相对路径拼接
    format!("{}/{}", base_url.trim_end_matches('/'), control.trim_start_matches('/'))
}
```

### 3.6 缺失采样率默认值填充

**范围**：`cheetah-rtsp-core` SDP 解析

**问题**：部分 IP 摄像头 SDP 中缺少视频 clock rate 或音频 sample rate

**实现**：

1. SDP 解析时的默认值填充规则：
   | 编码 | 默认 clock rate |
   |------|----------------|
   | H.264/H.265/VP8/VP9/AV1 | 90000 |
   | AAC | 从 fmtp config 解析，回退 44100 |
   | Opus | 48000 |
   | G.711A/G.711U | 8000 |
   | MP3 | 90000（RTP clock）|

2. 在 `core/src/compat/sdp_quirks.rs` 集中管理：
   ```rust
   pub fn default_clock_rate(codec: CodecId) -> u32 {
       match codec {
           CodecId::H264 | CodecId::H265 | CodecId::Vp8 | CodecId::Vp9 | CodecId::Av1 => 90000,
           CodecId::Aac => 44100,
           CodecId::Opus => 48000,
           CodecId::Pcma | CodecId::Pcmu => 8000,
           CodecId::Mp3 => 90000,
           _ => 90000,
       }
   }
   ```

3. 解析 SDP 时如果 `rtpmap` 行缺少 clock rate，使用默认值并记录 warn 日志

### 3.7 其他厂商 Quirks 集合

**范围**：`cheetah-rtsp-core` compat 模块

| Quirk | 描述 | 处理 |
|-------|------|------|
| 海康 H.265 SDP | `a=rtpmap` 使用 `H265` 而非标准 `H265/90000` | 容忍缺失 clock rate |
| 大华多 fmtp | 同一 track 多个 `a=fmtp` 行 | 合并所有 fmtp 参数 |
| ONVIF backchannel | `a=sendonly` 标记的音频回传轨 | 识别但不处理（记录日志） |
| 非标 session ID | 部分设备返回超长或含特殊字符的 Session ID | 放宽 Session ID 验证 |
| Range 格式 | `npt=now-` 而非标准 `npt=0.000-` | 解析 `now` 为 live 模式 |
| RTCP 端口 | 部分设备 RTCP 端口不是 RTP+1 | 从 Transport 头显式解析 |

每个 quirk 实现为独立函数，统一在 `compat/mod.rs` 注册。

## 配置汇总

```yaml
compat:
  strip_sdp_suffix: true
  heartbeat_mode: auto
  continue_push_ms: 10000
  default_video_clock_rate: 90000
  transport_case_insensitive: true
  allow_missing_unicast_tag: true
  session_id_max_length: 256
  merge_duplicate_fmtp: true
```

## 测试计划

| 测试类型 | 内容 |
|----------|------|
| 单元测试 | 每个 quirk 函数的正确性 |
| 回归测试 | 真实设备 SDP 样本解析（海康/大华/Axis/Bosch） |
| 集成测试 | 断连续推：推流断开→10s 内重连→订阅者不断流 |
| 集成测试 | Transport 461 降级：UDP 失败→自动切 TCP |
| 属性测试 | 任意格式 control URL 解析不 panic |
| fuzz | 非标 SDP 解析不崩溃 |

## 完成标准

- [ ] 海康/大华 IP 摄像头 RTSP 拉流成功
- [ ] EasyDarwin `.sdp` 后缀推流正常
- [ ] 断连续推 10s 内重连，订阅者无感知
- [ ] Transport 降级重试自动完成
- [ ] 所有 quirk 有独立开关和回归测试
