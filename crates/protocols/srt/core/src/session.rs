use bytes::Bytes;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SrtSessionId(pub u64);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SrtStatsSnapshot {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub packets_in: u64,
    pub packets_out: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrtCoreCommand {
    SendPayload { payload: Bytes },
    Close { reason: String },
}

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
