use crate::prelude::*;

/// 16-bit RTP sequence numbers wrap every 2^16 packets.
///
/// RTP 16 位序列号每 2^16 个包回绕一次。
const SEQUENCE_PERIOD: u64 = 1_u64 << 16;

/// Packets more than half a period ahead of the current expected sequence number
/// are treated as too far to be reliably reordered.
///
/// 超过当前期望序列号半周期以上的包被视为太远，无法可靠重排。
const HALF_SEQUENCE_PERIOD: i128 = 1_i128 << 15;

/// Settings for the RTP sequence-number reorder buffer.
///
/// RTP 序列号重排缓冲区的配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpReorderSettings {
    /// Maximum number of out-of-order packets held before forced release.
    ///
    /// 强制释放前缓存的乱序包数量上限。
    pub max_packets: usize,
    /// Maximum time an out-of-order packet may wait for its predecessors.
    ///
    /// 乱序包等待前驱包的最大时间（毫秒）。
    pub max_delay_ms: u64,
}

impl Default for RtpReorderSettings {
    fn default() -> Self {
        Self {
            max_packets: 2,
            max_delay_ms: 40,
        }
    }
}

#[derive(Debug, Clone)]
struct PendingPacket<T> {
    sequence_number: u64,
    arrival_ms: u64,
    packet: T,
}

/// Generic RTP sequence-number reorder buffer with bounded latency.
///
/// Sequence numbers are extended from 16-bit to monotonic 64-bit values before
/// any comparison, so wrap-around, duplicate detection and reorder windows are
/// all handled in the same unified path.
///
/// 具有有界延迟的通用 RTP 序列号重排缓冲区。
///
/// 序列号会先被扩展为单调递增的 64 位值，回绕、重复检测与乱序窗口都在同一条
/// 统一路径中处理。
#[derive(Debug, Clone)]
pub struct RtpReorderBuffer<T> {
    settings: RtpReorderSettings,
    expected_seq: Option<u64>,
    pending: Vec<PendingPacket<T>>,
}

impl<T> RtpReorderBuffer<T> {
    /// Create a new reorder buffer with the given settings.
    ///
    /// 使用给定配置创建新的重排缓冲区。
    pub fn new(settings: RtpReorderSettings) -> Self {
        Self {
            settings,
            expected_seq: None,
            pending: Vec::new(),
        }
    }

    /// Insert a packet by sequence number and arrival time, returning any packets
    /// that are now in order and ready for release.
    ///
    /// 按序列号和到达时间插入包，返回已形成顺序并可释放的包。
    pub fn push(&mut self, sequence_number: u16, arrival_ms: u64, packet: T) -> Vec<T> {
        let seq = self.extend_sequence(sequence_number);

        let Some(expected) = self.expected_seq else {
            self.expected_seq = Some(seq + 1);
            return vec![packet];
        };

        if seq < expected {
            return Vec::new();
        }

        if seq == expected {
            let mut out = vec![packet];
            self.expected_seq = Some(expected + 1);
            self.drain_contiguous(&mut out);
            return out;
        }

        // seq > expected: future packet, buffer for reordering.
        if !self.pending.iter().any(|p| p.sequence_number == seq) {
            self.pending.push(PendingPacket {
                sequence_number: seq,
                arrival_ms,
                packet,
            });
        }

        if self.settings.max_packets > 0 && self.pending.len() > self.settings.max_packets {
            return self.force_release_closest(expected);
        }

        if self.settings.max_delay_ms > 0
            && self
                .pending
                .iter()
                .any(|p| arrival_ms.saturating_sub(p.arrival_ms) >= self.settings.max_delay_ms)
        {
            return self.force_release_closest(expected);
        }

        // Hard cap: prevent unbounded growth when both max_packets and
        // max_delay_ms are zero (or when neither condition fires).
        const ABSOLUTE_PENDING_CAP: usize = 64;
        if self.pending.len() > ABSOLUTE_PENDING_CAP {
            return self.force_release_closest(expected);
        }

        Vec::new()
    }

    /// Reset the buffer, discarding all pending packets and expected state.
    ///
    /// 重置缓冲区，丢弃所有待处理包与期望状态。
    pub fn reset(&mut self) {
        self.expected_seq = None;
        self.pending.clear();
    }

    /// Number of packets currently buffered waiting for their predecessors.
    ///
    /// 当前缓存中等待前驱包的数量。
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Extend a raw 16-bit sequence number to a monotonic 64-bit value relative
    /// to the current expected sequence number.
    ///
    /// Returns `None` when the closest cycle is more than half a period away,
    /// which means the packet is either a late duplicate or too far ahead to be
    /// reordered reliably.
    fn extend_sequence(&self, raw: u16) -> u64 {
        let raw64 = u64::from(raw);
        let Some(expected) = self.expected_seq else {
            return raw64;
        };

        let period = i128::from(SEQUENCE_PERIOD);
        let exp = i128::from(expected);
        let cycle = i128::from(expected / SEQUENCE_PERIOD);
        let base = cycle * period + i128::from(raw64);

        let mut best_candidate = base;
        let mut best_diff = (base - exp).abs();
        for k in [-1_i128, 1] {
            let candidate = base + k * period;
            let diff = (candidate - exp).abs();
            if diff < best_diff {
                best_candidate = candidate;
                best_diff = diff;
            }
        }

        // Packets more than half a period ahead are treated as too far; still
        // return the closest extended value and let the caller decide. The push
        // method then compares against `expected` and drops anything behind it.
        if best_diff >= HALF_SEQUENCE_PERIOD {
            // Prefer the cycle that is closest to expected; if that is in the past,
            // push will drop it. If it is in the future by more than half a period,
            // we still deliver it as a (large) future packet and let the gap policy
            // flush pending.
            if best_candidate < exp {
                return expected.saturating_sub(1);
            }
        }

        best_candidate as u64
    }

    fn drain_contiguous(&mut self, out: &mut Vec<T>) {
        let mut expected = self.expected_seq.unwrap_or(0);
        while let Some(packet) = self.remove_pending(expected) {
            out.push(packet);
            expected += 1;
        }
        self.expected_seq = Some(expected);
    }

    fn force_release_closest(&mut self, expected: u64) -> Vec<T> {
        let Some((index, _distance)) = self
            .pending
            .iter()
            .enumerate()
            .map(|(idx, p)| (idx, p.sequence_number - expected))
            .min_by_key(|(_, distance)| *distance)
        else {
            return Vec::new();
        };

        let first_pending = self.pending.remove(index);
        let first_seq = first_pending.sequence_number;
        let first = first_pending.packet;
        let mut out = vec![first];
        self.expected_seq = Some(first_seq + 1);
        self.drain_contiguous(&mut out);
        out
    }

    fn remove_pending(&mut self, seq: u64) -> Option<T> {
        let idx = self
            .pending
            .iter()
            .position(|pending| pending.sequence_number == seq)?;
        Some(self.pending.remove(idx).packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(v: u16) -> (u16, u64, u16) {
        (v, u64::from(v), v)
    }

    #[test]
    fn orders_small_out_of_order_packets() {
        let mut reorder = RtpReorderBuffer::new(RtpReorderSettings {
            max_packets: 2,
            max_delay_ms: 100,
        });

        let (seq0, ts0, pkt0) = seq(100);
        assert_eq!(reorder.push(seq0, ts0, pkt0), vec![100]);

        let (seq2, ts2, pkt2) = seq(102);
        assert!(reorder.push(seq2, ts2, pkt2).is_empty());

        let (seq1, ts1, pkt1) = seq(101);
        assert_eq!(reorder.push(seq1, ts1, pkt1), vec![101, 102]);
    }

    #[test]
    fn buffers_forward_out_of_order_packets_beyond_plus_one() {
        let mut reorder = RtpReorderBuffer::new(RtpReorderSettings {
            max_packets: 8,
            max_delay_ms: 100,
        });

        assert_eq!(reorder.push(100, 1, 100), vec![100]);
        assert!(reorder.push(103, 2, 103).is_empty());
        assert_eq!(reorder.pending_len(), 1);
        assert_eq!(reorder.push(101, 3, 101), vec![101]);
        assert_eq!(reorder.push(102, 4, 102), vec![102, 103]);
    }

    #[test]
    fn drops_duplicate_pending_packet() {
        let mut reorder = RtpReorderBuffer::new(RtpReorderSettings {
            max_packets: 8,
            max_delay_ms: 100,
        });

        assert_eq!(reorder.push(100, 1, 100), vec![100]);
        assert!(reorder.push(102, 2, 102).is_empty());
        assert!(reorder.push(102, 3, 1020).is_empty());
        assert_eq!(reorder.pending_len(), 1);
        assert_eq!(reorder.push(101, 4, 101), vec![101, 102]);
    }

    #[test]
    fn drops_duplicate_already_delivered_packet() {
        let mut reorder = RtpReorderBuffer::new(RtpReorderSettings {
            max_packets: 8,
            max_delay_ms: 100,
        });

        assert_eq!(reorder.push(100, 1, 100), vec![100]);
        assert!(reorder.push(100, 2, 100).is_empty());
        assert_eq!(reorder.push(101, 3, 101), vec![101]);
    }

    #[test]
    fn drops_duplicate_from_previous_cycle_after_wraparound() {
        let mut reorder = RtpReorderBuffer::new(RtpReorderSettings {
            max_packets: 4,
            max_delay_ms: 100,
        });

        // Deliver the last sequence number of cycle 0 and wrap to 0.
        assert_eq!(reorder.push(u16::MAX, 1, u16::MAX), vec![u16::MAX]);
        assert_eq!(reorder.push(0, 2, 0), vec![0]);

        // Receiving u16::MAX again is a duplicate of the already-delivered
        // previous-cycle packet, not a new packet.
        assert!(reorder.push(u16::MAX, 3, u16::MAX).is_empty());

        // Continue normally.
        assert_eq!(reorder.push(1, 4, 1), vec![1]);
    }

    #[test]
    fn releases_when_gap_exceeds_packet_window() {
        let mut reorder = RtpReorderBuffer::new(RtpReorderSettings {
            max_packets: 2,
            max_delay_ms: 1_000,
        });

        assert_eq!(reorder.push(100, 1, 100), vec![100]);
        assert!(reorder.push(102, 2, 102).is_empty());
        assert!(reorder.push(104, 3, 104).is_empty());
        assert!(!reorder.push(105, 4, 105).is_empty());
    }

    #[test]
    fn releases_when_pending_delay_exceeds_threshold() {
        let mut reorder = RtpReorderBuffer::new(RtpReorderSettings {
            max_packets: 8,
            max_delay_ms: 5,
        });

        assert_eq!(reorder.push(100, 100, 100), vec![100]);
        assert!(reorder.push(102, 101, 102).is_empty());
        let released = reorder.push(103, 108, 103);
        assert!(!released.is_empty());
    }

    #[test]
    fn handles_sequence_wraparound() {
        let mut reorder = RtpReorderBuffer::new(RtpReorderSettings {
            max_packets: 4,
            max_delay_ms: 100,
        });

        assert_eq!(reorder.push(u16::MAX, 1, u16::MAX), vec![u16::MAX]);
        assert!(reorder.push(1, 2, 1).is_empty());
        assert_eq!(reorder.push(0, 3, 0), vec![0, 1]);
    }

    #[test]
    fn reset_clears_pending_state() {
        let mut reorder = RtpReorderBuffer::new(RtpReorderSettings {
            max_packets: 8,
            max_delay_ms: 100,
        });

        assert_eq!(reorder.push(100, 1, 100), vec![100]);
        assert!(reorder.push(102, 2, 102).is_empty());
        reorder.reset();
        assert_eq!(reorder.push(101, 3, 101), vec![101]);
    }
}
