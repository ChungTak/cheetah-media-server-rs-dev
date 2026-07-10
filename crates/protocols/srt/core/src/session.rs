use bytes::Bytes;

/// Identifier for `SRT Session`.
/// `SRT Session` 的标识符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SrtSessionId(pub u64);

/// `SrtStatsSnapshot` data structure.
/// `SrtStatsSnapshot` 数据结构。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SrtStatsSnapshot {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub packets_in: u64,
    pub packets_out: u64,
}

/// Command for `SRT Core`.
/// `SRT Core` 的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrtCoreCommand {
    SendPayload { payload: Bytes },
    Close { reason: String },
}

/// `SrtCoreInput` enumeration.
/// `SrtCoreInput` 枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// `SrtCoreOutput` enumeration.
/// `SrtCoreOutput` 枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Events produced by the `SRT Core` subsystem.
/// `SRT Core` 子系统产生的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
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
