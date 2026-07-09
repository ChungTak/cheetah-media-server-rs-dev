# RTMP Phase 04 — 播放控制与协议扩展

- **状态**: 未开始
- **范围**: 服务端/客户端 Seek/Pause/Speed、聚合消息 type 22、Multi-track Enhanced RTMP
- **完成标准**: VOD 流支持 seek/pause/speed 控制，聚合消息可正确解析，multi-track 信令可识别

---

## 目标

1. 支持 VOD 播放场景的 Seek/Pause/Speed 控制
2. 解析 RTMP 聚合消息（type 22），提升与旧版服务器的兼容性
3. 支持 Enhanced RTMP Multi-track 信令

---

## 设计约束

- Seek/Pause/Speed 命令解析在 `cheetah-rtmp-core`
- VOD 数据源由录制模块或外部文件提供，module 层协调
- 聚合消息解析在 core 层，拆分为独立子消息后正常处理
- Multi-track 解析在 core 层，module 层决定如何路由多轨道

---

## 任务分解

### 4.1 服务端 Seek/Pause/Speed 命令

**目标**: 服务端正确解析和响应播放控制命令。

**实现**:

1. `cheetah-rtmp-core` 命令解析扩展：

```rust
/// 新增命令解析
fn parse_command(name: &str, args: &[AmfValue]) -> Option<CoreInput> {
    match name {
        "seek" => {
            let millis = args.get(1)?.as_number()?;
            Some(CoreInput::SeekCommand { stream_id, millis })
        }
        "pause" => {
            let pause = args.get(1)?.as_bool()?;
            let millis = args.get(2)?.as_number().unwrap_or(0.0);
            Some(CoreInput::PauseCommand { stream_id, pause, millis })
        }
        "receiveVideo" => {
            let enabled = args.get(1)?.as_bool()?;
            Some(CoreInput::ReceiveVideo { stream_id, enabled })
        }
        "receiveAudio" => {
            let enabled = args.get(1)?.as_bool()?;
            Some(CoreInput::ReceiveAudio { stream_id, enabled })
        }
        _ => None,
    }
}
```

2. `cheetah-rtmp-core` 输出扩展：

```rust
pub enum CoreOutput {
    // ... 已有
    /// 播放者请求 seek
    SeekRequested { stream_id: u32, millis: f64 },
    /// 播放者请求 pause/unpause
    PauseRequested { stream_id: u32, pause: bool, millis: f64 },
    /// 播放者请求 receiveVideo 开关
    ReceiveVideoToggled { stream_id: u32, enabled: bool },
    /// 播放者请求 receiveAudio 开关
    ReceiveAudioToggled { stream_id: u32, enabled: bool },
    /// 发送 StreamEOF user control
    SendStreamEof { stream_id: u32 },
    /// 发送 StreamBegin user control
    SendStreamBegin { stream_id: u32 },
}
```

3. Module 层处理：

```rust
fn handle_seek(&mut self, session_id: SessionId, millis: f64) {
    // 查找该 session 订阅的流
    let stream_key = self.get_subscribed_stream(session_id);

    // 通知数据源 seek（仅 VOD 流支持）
    if let Some(vod_source) = self.get_vod_source(&stream_key) {
        vod_source.seek(millis);
        // 发送 StreamEOF → 清空缓冲 → 从新位置发送 → StreamBegin
        self.send_user_control(session_id, UserControl::StreamEof);
        self.send_user_control(session_id, UserControl::StreamBegin);
    } else {
        // 直播流不支持 seek，发送 NetStream.Seek.Failed
        self.send_status(session_id, "NetStream.Seek.Failed", "Live stream cannot seek");
    }
}

fn handle_pause(&mut self, session_id: SessionId, pause: bool) {
    if pause {
        self.pause_subscription(session_id);
        self.send_user_control(session_id, UserControl::StreamEof);
        self.send_status(session_id, "NetStream.Pause.Notify", "Paused");
    } else {
        self.resume_subscription(session_id);
        self.send_user_control(session_id, UserControl::StreamBegin);
        self.send_status(session_id, "NetStream.Unpause.Notify", "Unpaused");
    }
}
```

**测试**:
- 单元测试：seek/pause 命令 AMF 解析
- 单元测试：状态机正确生成 SeekRequested/PauseRequested 输出
- 集成测试：播放器发送 pause → 流暂停 → unpause → 流恢复

---

### 4.2 客户端 Seek/Pause/Speed

**目标**: RTMP 客户端（pull player）支持发送 seek/pause/speed 命令。

**实现**:

1. `cheetah-rtmp-core` 客户端命令生成：

```rust
pub enum ClientCommand {
    // ... 已有 (Play, Publish)
    Seek { millis: f64 },
    Pause { pause: bool, millis: f64 },
}

impl RtmpClientState {
    /// 生成 seek 命令包
    pub fn encode_seek(&mut self, millis: f64) -> Vec<u8> {
        let mut encoder = AmfEncoder::new();
        encoder.write_string("seek");
        encoder.write_number(0.0); // transaction id
        encoder.write_null();
        encoder.write_number(millis);
        self.encode_command(encoder.into_bytes())
    }

    /// 生成 pause 命令包
    pub fn encode_pause(&mut self, pause: bool, millis: f64) -> Vec<u8> {
        let mut encoder = AmfEncoder::new();
        encoder.write_string("pause");
        encoder.write_number(0.0);
        encoder.write_null();
        encoder.write_bool(pause);
        encoder.write_number(millis);
        self.encode_command(encoder.into_bytes())
    }
}
```

2. Driver 层暴露控制接口：

```rust
/// 客户端连接控制句柄
pub struct RtmpClientHandle {
    command_tx: mpsc::Sender<ClientCommand>,
}

impl RtmpClientHandle {
    pub async fn seek(&self, millis: f64) -> Result<()> {
        self.command_tx.send(ClientCommand::Seek { millis }).await?;
        Ok(())
    }

    pub async fn pause(&self, pause: bool) -> Result<()> {
        self.command_tx.send(ClientCommand::Pause { pause, millis: 0.0 }).await?;
        Ok(())
    }
}
```

**测试**:
- 单元测试：seek/pause 命令编码正确性
- 集成测试：客户端 seek → 服务端响应 → 数据从新位置开始

---

### 4.3 聚合消息 Type 22 解析

**目标**: 正确解析 RTMP 聚合消息，拆分为独立子消息处理。

**实现**:

1. `cheetah-rtmp-core` 消息解析扩展：

```rust
/// RTMP 聚合消息 (type 22)
/// 格式: [tag_type(1) + data_size(3) + timestamp(3) + timestamp_ext(1) + stream_id(3) + data(N) + prev_tag_size(4)]*
pub fn parse_aggregate_message(data: &[u8]) -> Vec<SubMessage> {
    let mut messages = Vec::new();
    let mut offset = 0;

    while offset + 11 <= data.len() {
        let tag_type = data[offset];
        let data_size = read_u24_be(&data[offset + 1..]);
        let timestamp = read_u24_be(&data[offset + 4..]);
        let timestamp_ext = data[offset + 7];
        let full_timestamp = ((timestamp_ext as u32) << 24) | timestamp;
        // stream_id: data[offset + 8..offset + 11] (always 0)

        let payload_start = offset + 11;
        let payload_end = payload_start + data_size as usize;

        if payload_end > data.len() {
            break; // 截断的聚合消息，停止解析
        }

        messages.push(SubMessage {
            msg_type: tag_type,
            timestamp: full_timestamp,
            payload: &data[payload_start..payload_end],
        });

        // skip prev_tag_size (4 bytes)
        offset = payload_end + 4;
    }

    messages
}
```

2. 集成到状态机：

```rust
fn handle_message(&mut self, msg: RtmpMessage) -> Vec<CoreOutput> {
    match msg.msg_type {
        22 => {
            // 聚合消息：拆分后逐个处理
            let sub_messages = parse_aggregate_message(&msg.payload);
            let mut outputs = Vec::new();
            for sub in sub_messages {
                let sub_msg = RtmpMessage {
                    msg_type: sub.msg_type,
                    timestamp: sub.timestamp,
                    payload: sub.payload.to_vec(),
                    ..msg.clone()
                };
                outputs.extend(self.handle_message(sub_msg));
            }
            outputs
        }
        // ... 其他消息类型
    }
}
```

**测试**:
- 单元测试：聚合消息解析（正常、空、截断）
- 属性测试：任意字节序列解析不 panic
- 集成测试：发送聚合消息 → 服务器正确处理子消息

---

### 4.4 Multi-track Enhanced RTMP

**目标**: 支持 Enhanced RTMP 规范中的 Multi-track 信令。

**实现**:

1. `cheetah-rtmp-core` Multi-track 解析：

```rust
/// Enhanced RTMP Multi-track 包类型
/// PacketType = MultiTrack (6)
pub struct MultiTrackPacket {
    pub multi_track_type: MultiTrackType,
    pub tracks: Vec<TrackEntry>,
}

pub enum MultiTrackType {
    OneTrack,
    ManyTracks,
    ManyTracksManyCodecs,
}

pub struct TrackEntry {
    pub track_id: u8,
    pub codec_fourcc: [u8; 4],  // 仅 ManyTracksManyCodecs 时有效
    pub data: Vec<u8>,
}

/// 解析 multi-track 包
pub fn parse_multi_track(data: &[u8]) -> Option<MultiTrackPacket> {
    let packet_type = data[0] & 0x0F;
    if packet_type != 6 { return None; } // PacketType::MultiTrack

    let multi_track_type = match (data[1] >> 4) & 0x0F {
        0 => MultiTrackType::OneTrack,
        1 => MultiTrackType::ManyTracks,
        2 => MultiTrackType::ManyTracksManyCodecs,
        _ => return None,
    };

    // 解析各 track entry...
    todo!()
}
```

2. Module 层路由：

```rust
fn handle_multi_track(&mut self, stream_key: &StreamKey, packet: MultiTrackPacket) {
    for track in packet.tracks {
        // 每个 track 作为独立的 TrackInfo 发布到引擎
        let track_key = format!("{}/track_{}", stream_key, track.track_id);
        self.engine.publish_track(&track_key, track.data);
    }
}
```

3. 设计要点：
   - Multi-track 是 Enhanced RTMP v2 的可选扩展
   - 当前阶段仅实现解析和识别，不强制要求完整的多轨道路由
   - 单轨道场景（大多数情况）不受影响

**测试**:
- 单元测试：multi-track 包解析
- 单元测试：各 MultiTrackType 的正确处理
- 集成测试：multi-track 推流 → 多轨道正确识别
