//! RTP sequence number tracking: wraparound detection, reset detection, and loss statistics.
//!
//! This is a pure state machine: it accepts the next 16-bit sequence number and
//! reports whether the packet is in-order, a duplicate, a wrap, a gap, or a
//! sender reset. It contains no timers or I/O.
//!
//! RTP 序列号追踪：回绕检测、重启检测和丢包统计。
//!
//! 这是一个纯状态机：接收下一个 16 位序列号并报告该包是顺序、重复、回绕、间隙还是
//! 发送者重启。不包含定时器或 I/O。

/// Events detected by the sequence tracker.
///
/// The tracker reports the relationship of each incoming sequence number to the
/// expected one. `Gap` and `Reset` are the primary events that the driver should
/// act on.
///
/// 序列追踪器检测到的事件。
///
/// 追踪器报告每个到达序列号与预期序列号的关系。`Gap` 和 `Reset` 是驱动层应主要
/// 处理的事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqEvent {
    /// First packet seen — no prior state.
    ///
    /// 第一个到达的包——没有先前状态。
    Initial,
    /// Normal in-order packet (seq == expected).
    ///
    /// 正常顺序包（seq == 预期值）。
    Normal,
    /// Normal 16-bit wraparound (65535 → 0).
    ///
    /// 正常 16 位回绕（65535 → 0）。
    Wrap,
    /// Sender restart detected (large seq jump beyond threshold).
    ///
    /// 检测到发送者重启（超过阈值的序列号跳跃）。
    Reset,
    /// Duplicate packet (same seq as last delivered).
    ///
    /// 重复包（与上一个已交付序列号相同）。
    Duplicate,
    /// Out-of-order packet (seq < expected but within threshold).
    ///
    /// 乱序包（seq < 预期值但在阈值内）。
    OutOfOrder,
    /// Forward gap (seq > expected but within threshold — lost packets).
    ///
    /// 前向间隙（seq > 预期值但在阈值内——存在丢包）。
    Gap { lost: u16 },
}

/// Tracks RTP sequence numbers for a single SSRC.
///
/// Detects normal wraparound vs sender restart, counts losses, and derives an
/// extended 32-bit sequence number. This is Sans-I/O: no timers, no I/O, no async.
///
/// 为单个 SSRC 追踪 RTP 序列号。
///
/// 区分正常回绕与发送者重启、统计丢包并派生扩展 32 位序列号。这是 Sans-I/O：
/// 无定时器、无 I/O、无异步。
#[derive(Debug, Clone)]
pub struct SeqTracker {
    last_seq: Option<u16>,
    wrap_count: u32,
    total_packets: u64,
    total_lost: u64,
    /// Seq jump beyond this threshold is considered a sender reset.
    ///
    /// 超过此阈值的序列号跳跃被视为发送者重启。
    reset_threshold: u16,
}

impl SeqTracker {
    /// Create a new tracker with the default reset threshold (5000).
    ///
    /// 创建以默认重启阈值 5000 开头的追踪器。
    pub fn new() -> Self {
        Self::with_threshold(5000)
    }

    /// Create a tracker with a custom reset threshold.
    ///
    /// A smaller threshold makes the tracker more sensitive to jumps; a larger
    /// one tolerates larger reordering windows.
    ///
    /// 以自定义重启阈值创建追踪器。
    ///
    /// 更小的阈值使追踪器对跳跃更敏感；更大的阈值容忍更大的重排窗口。
    pub fn with_threshold(reset_threshold: u16) -> Self {
        Self {
            last_seq: None,
            wrap_count: 0,
            total_packets: 0,
            total_lost: 0,
            reset_threshold,
        }
    }

    /// Process an incoming sequence number and return the detected event.
    ///
    /// The algorithm compares the new sequence to the expected value (`last + 1`)
    /// using `u16::wrapping_sub` to handle the 16-bit ring. Forward distances
    /// below 0x8000 are treated as gaps or resets; distances above 0x8000 are
    /// treated as out-of-order or backward resets.
    ///
    /// 处理一个到达的序列号并返回检测到的事件。
    ///
    /// 算法使用 `u16::wrapping_sub` 将新序列与预期值（`last + 1`）比较，以处理 16 位环。
    /// 小于 0x8000 的前向距离视为间隙或重启；大于 0x8000 的距离视为乱序或向后重启。
    pub fn update(&mut self, seq: u16) -> SeqEvent {
        self.total_packets += 1;

        let Some(last) = self.last_seq else {
            self.last_seq = Some(seq);
            return SeqEvent::Initial;
        };

        let expected = last.wrapping_add(1);

        if seq == expected {
            self.last_seq = Some(seq);
            if expected == 0 {
                self.wrap_count += 1;
                return SeqEvent::Wrap;
            }
            return SeqEvent::Normal;
        }

        if seq == last {
            return SeqEvent::Duplicate;
        }

        // Forward distance (how far ahead is seq from expected)
        let forward = seq.wrapping_sub(expected);
        // Backward distance (how far behind is seq from expected)
        let backward = expected.wrapping_sub(seq);

        if forward < 0x8000 {
            // seq is ahead of expected
            if forward >= self.reset_threshold {
                // Too large a jump — sender reset
                self.last_seq = Some(seq);
                self.wrap_count = 0;
                return SeqEvent::Reset;
            }
            // Normal gap (lost packets)
            let lost = forward;
            self.total_lost += u64::from(lost);
            self.last_seq = Some(seq);
            if last > seq {
                // Crossed the 16-bit boundary
                self.wrap_count += 1;
            }
            SeqEvent::Gap { lost }
        } else {
            // seq is behind expected — out of order or very late
            if backward >= self.reset_threshold {
                // Very large backward jump — also treat as reset
                self.last_seq = Some(seq);
                self.wrap_count = 0;
                return SeqEvent::Reset;
            }
            SeqEvent::OutOfOrder
        }
    }

    /// Extended sequence number (32-bit) accounting for wraps.
    ///
    /// 计算考虑回绕后的扩展序列号（32 位）。
    pub fn extended_seq(&self) -> u64 {
        let seq = u64::from(self.last_seq.unwrap_or(0));
        u64::from(self.wrap_count) * 65536 + seq
    }

    /// Total number of packets processed by `update`.
    ///
    /// 经 `update` 处理的总包数。
    pub fn total_packets(&self) -> u64 {
        self.total_packets
    }

    /// Total number of detected lost packets.
    ///
    /// 检测到的总丢包数。
    pub fn total_lost(&self) -> u64 {
        self.total_lost
    }

    /// Number of 16-bit wrap cycles observed.
    ///
    /// 观察到的 16 位回绕周期数。
    pub fn wrap_count(&self) -> u32 {
        self.wrap_count
    }

    /// Fraction of packets lost on a 0..256 scale, suitable for RTCP RR.
    ///
    /// This is an approximation because duplicates and out-of-order packets are
    /// also counted in `total_packets`.
    ///
    /// 以 0..256 比例表示的丢包率，适用于 RTCP RR。
    ///
    /// 这是一个近似值，因为重复和乱序包也被计入 `total_packets`。
    pub fn fraction_lost_256(&self) -> u8 {
        if self.total_packets == 0 {
            return 0;
        }
        let expected = self.total_packets + self.total_lost;
        let frac = (self.total_lost * 256) / expected;
        frac.min(255) as u8
    }

    /// Reset all state (e.g., after a sender restart is detected externally).
    ///
    /// 重置所有状态（例如外部检测到发送者重启后）。
    pub fn reset(&mut self) {
        self.last_seq = None;
        self.wrap_count = 0;
        self.total_packets = 0;
        self.total_lost = 0;
    }
}

impl Default for SeqTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_packet() {
        let mut t = SeqTracker::new();
        assert_eq!(t.update(100), SeqEvent::Initial);
        assert_eq!(t.total_packets(), 1);
    }

    #[test]
    fn normal_sequence() {
        let mut t = SeqTracker::new();
        t.update(100);
        assert_eq!(t.update(101), SeqEvent::Normal);
        assert_eq!(t.update(102), SeqEvent::Normal);
    }

    #[test]
    fn normal_wraparound() {
        let mut t = SeqTracker::new();
        t.update(65534);
        assert_eq!(t.update(65535), SeqEvent::Normal);
        assert_eq!(t.update(0), SeqEvent::Wrap);
        assert_eq!(t.wrap_count(), 1);
        assert_eq!(t.update(1), SeqEvent::Normal);
    }

    #[test]
    fn gap_detection() {
        let mut t = SeqTracker::new();
        t.update(100);
        let event = t.update(105);
        assert_eq!(event, SeqEvent::Gap { lost: 4 });
        assert_eq!(t.total_lost(), 4);
    }

    #[test]
    fn duplicate_detection() {
        let mut t = SeqTracker::new();
        t.update(100);
        assert_eq!(t.update(100), SeqEvent::Duplicate);
    }

    #[test]
    fn out_of_order_detection() {
        let mut t = SeqTracker::new();
        t.update(100);
        t.update(101);
        t.update(102);
        assert_eq!(t.update(101), SeqEvent::OutOfOrder);
    }

    #[test]
    fn reset_detection_forward() {
        let mut t = SeqTracker::with_threshold(5000);
        t.update(100);
        assert_eq!(t.update(60000), SeqEvent::Reset);
    }

    #[test]
    fn reset_detection_backward() {
        let mut t = SeqTracker::with_threshold(5000);
        t.update(60000);
        t.update(60001);
        assert_eq!(t.update(100), SeqEvent::Reset);
    }

    #[test]
    fn gap_across_wrap_boundary() {
        let mut t = SeqTracker::new();
        t.update(65530);
        let event = t.update(2); // gap of 7 (65531..65535, 0, 1)
        assert_eq!(event, SeqEvent::Gap { lost: 7 });
        assert_eq!(t.wrap_count(), 1);
    }
}
