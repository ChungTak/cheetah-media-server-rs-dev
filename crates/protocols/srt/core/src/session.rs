use bytes::Bytes;

/// `SrtSessionId` data structure.
/// `SrtSessionId` 数据结构.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SrtSessionId(pub u64);

/// `SrtStatsSnapshot` data structure.
/// `SrtStatsSnapshot` 数据结构.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SrtStatsSnapshot {
    /// `bytes_in` field of type `u64`.
    /// `bytes_in` 字段，类型为 `u64`.
    pub bytes_in: u64,
    /// `bytes_out` field of type `u64`.
    /// `bytes_out` 字段，类型为 `u64`.
    pub bytes_out: u64,
    /// `packets_in` field of type `u64`.
    /// `packets_in` 字段，类型为 `u64`.
    pub packets_in: u64,
    /// `packets_out` field of type `u64`.
    /// `packets_out` 字段，类型为 `u64`.
    pub packets_out: u64,
}

/// `SrtCoreCommand` enumeration.
/// `SrtCoreCommand` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrtCoreCommand {
    /// `SendPayload` variant.
    /// `SendPayload` 变体.
    SendPayload { payload: Bytes },
    /// `Close` variant.
    /// `Close` 变体.
    Close { reason: String },
}

/// `SrtCoreInput` enumeration.
/// `SrtCoreInput` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrtCoreInput {
    /// `Packet` variant.
    /// `Packet` 变体.
    Packet { now_micros: u64, bytes: Bytes },
    /// `SendPayload` variant.
    /// `SendPayload` 变体.
    SendPayload { now_micros: u64, payload: Bytes },
    /// `Timer` variant.
    /// `Timer` 变体.
    Timer {
        now_micros: u64,
        timer_id: shiguredo_srt::TimerId,
    },
    /// `Close` variant.
    /// `Close` 变体.
    Close { now_micros: u64, reason: String },
}

/// `SrtCoreOutput` enumeration.
/// `SrtCoreOutput` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrtCoreOutput {
    /// `SendPacket` variant.
    /// `SendPacket` 变体.
    SendPacket { bytes: Bytes },
    /// `SetTimer` variant.
    /// `SetTimer` 变体.
    SetTimer {
        timer_id: shiguredo_srt::TimerId,
        duration_micros: u64,
    },
    /// `ClearTimer` variant.
    /// `ClearTimer` 变体.
    ClearTimer { timer_id: shiguredo_srt::TimerId },
    /// `Event` variant.
    /// `Event` 变体.
    Event(SrtCoreEvent),
}

/// `SrtCoreEvent` enumeration.
/// `SrtCoreEvent` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrtCoreEvent {
    /// `Connected` variant.
    /// `Connected` 变体.
    Connected,
    /// `PayloadReceived` variant.
    /// `PayloadReceived` 变体.
    PayloadReceived {
        payload: Bytes,
        message_number: u32,
        timestamp: u32,
    },
    /// `KeyRefreshNeeded` variant.
    /// `KeyRefreshNeeded` 变体.
    KeyRefreshNeeded { key_length: usize },
    /// `Disconnected` variant.
    /// `Disconnected` 变体.
    Disconnected { reason: String },
    /// `Error` variant.
    /// `Error` 变体.
    Error { message: String },
    /// `Stats` variant.
    /// `Stats` 变体.
    Stats { snapshot: SrtStatsSnapshot },
}
