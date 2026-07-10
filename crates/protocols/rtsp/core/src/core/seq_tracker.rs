//! RTP sequence number tracking: wraparound detection, reset detection, and loss statistics.

/// Events detected by the sequence tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqEvent {
    /// First packet seen — no prior state.
    Initial,
    /// Normal in-order packet (seq == expected).
    Normal,
    /// Normal 16-bit wraparound (65535 → 0).
    Wrap,
    /// Sender restart detected (large seq jump beyond threshold).
    Reset,
    /// Duplicate packet (same seq as last delivered).
    Duplicate,
    /// Out-of-order packet (seq < expected but within threshold).
    OutOfOrder,
    /// Forward gap (seq > expected but within threshold — lost packets).
    Gap { lost: u16 },
}

/// Tracks RTP sequence numbers for a single SSRC.
///
/// Detects normal wraparound vs sender restart, counts losses.
/// Sans-I/O: no timers, no I/O, no async.
#[derive(Debug, Clone)]
pub struct SeqTracker {
    last_seq: Option<u16>,
    wrap_count: u32,
    total_packets: u64,
    total_lost: u64,
    /// Seq jump beyond this threshold is considered a sender reset.
    reset_threshold: u16,
}

impl SeqTracker {
    /// Create a new tracker with default reset threshold (5000).
    pub fn new() -> Self {
        Self::with_threshold(5000)
    }

    /// Create with custom reset threshold.
    pub fn with_threshold(reset_threshold: u16) -> Self {
        Self {
            last_seq: None,
            wrap_count: 0,
            total_packets: 0,
            total_lost: 0,
            reset_threshold,
        }
    }

    /// Process an incoming sequence number. Returns the detected event.
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
    pub fn extended_seq(&self) -> u64 {
        let seq = u64::from(self.last_seq.unwrap_or(0));
        u64::from(self.wrap_count) * 65536 + seq
    }

    pub fn total_packets(&self) -> u64 {
        self.total_packets
    }

    pub fn total_lost(&self) -> u64 {
        self.total_lost
    }

    pub fn wrap_count(&self) -> u32 {
        self.wrap_count
    }

    /// Fraction of packets lost (0..256 scale, for RTCP RR).
    /// Note: includes duplicates/OOO in total_packets count (approximation).
    pub fn fraction_lost_256(&self) -> u8 {
        if self.total_packets == 0 {
            return 0;
        }
        let expected = self.total_packets + self.total_lost;
        let frac = (self.total_lost * 256) / expected;
        frac.min(255) as u8
    }

    /// Reset all state (e.g., after detecting a sender restart externally).
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
