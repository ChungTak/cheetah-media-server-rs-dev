use bytes::Bytes;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Opaque identifier for an SRT session.
///
/// SRT 会话的不透明标识符。
pub struct SrtSessionId(pub u64);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Snapshot of per-session byte/packet counters.
///
/// 每会话字节/包计数器快照。
pub struct SrtStatsSnapshot {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub packets_in: u64,
    pub packets_out: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Commands sent from the module into the SRT core.
///
/// 从模块发送到 SRT core 的命令。
pub enum SrtCoreCommand {
    SendPayload { payload: Bytes },
    Close { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Inputs delivered to the SRT core state machine.
///
/// 递交给 SRT core 状态机的输入。
pub enum SrtCoreInput {
    Packet {
        now_micros: u64,
        bytes: Bytes,
    },
    SendPayload {
        now_micros: u64,
        payload: Bytes,
    },
    Timer {
        now_micros: u64,
        timer_id: shiguredo_srt::TimerId,
    },
    Close {
        now_micros: u64,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Outputs produced by the SRT core state machine.
///
/// SRT core 状态机产生的输出。
pub enum SrtCoreOutput {
    SendPacket {
        bytes: Bytes,
    },
    SetTimer {
        timer_id: shiguredo_srt::TimerId,
        duration_micros: u64,
    },
    ClearTimer {
        timer_id: shiguredo_srt::TimerId,
    },
    Event(SrtCoreEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Events surfaced by the SRT core to the module.
///
/// SRT core 向模块暴露的事件。
pub enum SrtCoreEvent {
    Connected,
    PayloadReceived {
        payload: Bytes,
        message_number: u32,
        timestamp: u32,
    },
    KeyRefreshNeeded {
        key_length: usize,
    },
    Disconnected {
        reason: String,
    },
    Error {
        message: String,
    },
    Stats {
        snapshot: SrtStatsSnapshot,
    },
}
