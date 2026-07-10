//! HLS player state machine for pull scenarios.
//!
//! Manages playlist refresh timing, segment deduplication, and adaptive bitrate selection.

use crate::parser::{ParsedMasterPlaylist, ParsedMediaPlaylist, ParsedVariant};

/// Adaptive bitrate selection strategy.
#[derive(Debug, Clone, Copy)]
pub enum BandwidthStrategy {
    Highest,
    Lowest,
    Auto { safety_factor: f64 },
}

/// HLS player state — tracks playlist refresh, segment dedup, and ABR.
pub struct HlsPlayerState {
    /// Selected variant URI (from master playlist).
    pub selected_variant_uri: Option<String>,
    /// Last seen media sequence number.
    pub last_sequence: i64,
    /// Set of segment URIs already downloaded (dedup).
    seen_uris: std::collections::HashSet<String>,
    /// Estimated bandwidth in bytes/sec (EWMA).
    estimated_bandwidth: u64,
    /// Target duration from playlist (ms).
    pub target_duration_ms: u64,
    /// Consecutive unchanged playlist reloads.
    unchanged_count: u8,
    /// Whether the stream is live.
    pub is_live: bool,
}

impl HlsPlayerState {
    /// Creates a new `HlsPlayerState` instance.
    /// 创建新的 `HlsPlayerState` 实例。
    pub fn new() -> Self {
        Self {
            selected_variant_uri: None,
            last_sequence: -1,
            seen_uris: std::collections::HashSet::new(),
            estimated_bandwidth: 0,
            target_duration_ms: 4000,
            unchanged_count: 0,
            is_live: true,
        }
    }

    /// Select a variant from a master playlist based on bandwidth strategy.
    pub fn select_variant<'a>(
        &mut self,
        master: &'a ParsedMasterPlaylist,
        strategy: BandwidthStrategy,
    ) -> Option<&'a ParsedVariant> {
        if master.variants.is_empty() {
            return None;
        }
        let variant = match strategy {
            BandwidthStrategy::Highest => master.variants.iter().max_by_key(|v| v.bandwidth),
            BandwidthStrategy::Lowest => master.variants.iter().min_by_key(|v| v.bandwidth),
            BandwidthStrategy::Auto { safety_factor } => {
                let threshold = (self.estimated_bandwidth as f64 * safety_factor) as u64;
                master
                    .variants
                    .iter()
                    .filter(|v| v.bandwidth <= threshold)
                    .max_by_key(|v| v.bandwidth)
                    .or_else(|| master.variants.iter().min_by_key(|v| v.bandwidth))
            }
        };
        if let Some(v) = variant {
            self.selected_variant_uri = Some(v.uri.clone());
        }
        variant
    }

    /// Process a newly fetched media playlist. Returns new segment URIs to download.
    pub fn on_playlist(&mut self, playlist: &ParsedMediaPlaylist) -> Vec<String> {
        self.target_duration_ms = playlist.target_duration as u64 * 1000;
        self.is_live = !playlist.end_list;

        let new_sequence = playlist.media_sequence as i64;
        if new_sequence > self.last_sequence {
            self.last_sequence = new_sequence;
            self.unchanged_count = 0;
        } else {
            self.unchanged_count = self.unchanged_count.saturating_add(1);
        }

        let mut new_segments = Vec::new();
        for seg in &playlist.segments {
            // Dedup by URI (ignore query params for comparison)
            let key = seg.uri.split('?').next().unwrap_or(&seg.uri).to_string();
            if self.seen_uris.insert(key) {
                new_segments.push(seg.uri.clone());
            }
        }

        // Prevent unbounded growth: keep only the most recent entries
        const MAX_SEEN_URIS: usize = 1024;
        if self.seen_uris.len() > MAX_SEEN_URIS {
            // Clear and re-populate with current playlist segments only
            self.seen_uris.clear();
            for seg in &playlist.segments {
                let key = seg.uri.split('?').next().unwrap_or(&seg.uri).to_string();
                self.seen_uris.insert(key);
            }
        }

        new_segments
    }

    /// Compute the delay before next playlist refresh (ms).
    pub fn refresh_delay_ms(&self) -> u64 {
        if !self.is_live {
            return 0; // VOD: no refresh needed
        }
        let base = self.target_duration_ms;
        if self.unchanged_count == 0 {
            base / 2 // Playlist changed: poll at half target duration
        } else {
            // Playlist unchanged: back off
            let multiplier = (self.unchanged_count as u64 + 1).min(5);
            base * multiplier / 2
        }
    }

    /// Update bandwidth estimate after a segment download.
    pub fn update_bandwidth(&mut self, bytes: u64, duration_ms: u64) {
        if duration_ms == 0 {
            return;
        }
        let bps = bytes * 1000 / duration_ms;
        if self.estimated_bandwidth == 0 {
            self.estimated_bandwidth = bps;
        } else {
            // EWMA with 7/8 weight on history
            self.estimated_bandwidth = (self.estimated_bandwidth * 7 + bps) / 8;
        }
    }

    /// Current estimated bandwidth (bytes/sec).
    pub fn bandwidth(&self) -> u64 {
        self.estimated_bandwidth
    }

    /// Number of consecutive unchanged playlist reloads.
    pub fn unchanged_count(&self) -> u8 {
        self.unchanged_count
    }
}

impl Default for HlsPlayerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{ParsedMediaPlaylist, ParsedSegment, ParsedVariant};

    #[test]
    fn select_highest_variant() {
        let master = ParsedMasterPlaylist {
            variants: vec![
                ParsedVariant {
                    bandwidth: 1000000,
                    uri: "low.m3u8".into(),
                },
                ParsedVariant {
                    bandwidth: 5000000,
                    uri: "high.m3u8".into(),
                },
            ],
        };
        let mut state = HlsPlayerState::new();
        let v = state
            .select_variant(&master, BandwidthStrategy::Highest)
            .unwrap();
        assert_eq!(v.uri, "high.m3u8");
    }

    #[test]
    fn on_playlist_deduplicates() {
        let mut state = HlsPlayerState::new();
        let pl = ParsedMediaPlaylist {
            target_duration: 4,
            media_sequence: 0,
            segments: vec![
                ParsedSegment {
                    duration: 4.0,
                    uri: "seg0.ts".into(),
                },
                ParsedSegment {
                    duration: 4.0,
                    uri: "seg1.ts".into(),
                },
            ],
            end_list: false,
        };
        let new1 = state.on_playlist(&pl);
        assert_eq!(new1.len(), 2);

        // Second call with same segments → no new
        let new2 = state.on_playlist(&pl);
        assert_eq!(new2.len(), 0);

        // Third call with one new segment
        let pl2 = ParsedMediaPlaylist {
            target_duration: 4,
            media_sequence: 1,
            segments: vec![
                ParsedSegment {
                    duration: 4.0,
                    uri: "seg1.ts".into(),
                },
                ParsedSegment {
                    duration: 4.0,
                    uri: "seg2.ts".into(),
                },
            ],
            end_list: false,
        };
        let new3 = state.on_playlist(&pl2);
        assert_eq!(new3.len(), 1);
        assert_eq!(new3[0], "seg2.ts");
    }

    #[test]
    fn refresh_delay_backs_off() {
        let mut state = HlsPlayerState::new();
        state.target_duration_ms = 4000;
        state.is_live = true;
        state.unchanged_count = 0;
        assert_eq!(state.refresh_delay_ms(), 2000);

        state.unchanged_count = 1;
        assert_eq!(state.refresh_delay_ms(), 4000);

        state.unchanged_count = 4;
        assert_eq!(state.refresh_delay_ms(), 10000);
    }

    #[test]
    fn bandwidth_ewma() {
        let mut state = HlsPlayerState::new();
        state.update_bandwidth(1_000_000, 1000); // 1MB/s
        assert_eq!(state.bandwidth(), 1_000_000);

        state.update_bandwidth(2_000_000, 1000); // 2MB/s
                                                 // EWMA: (1M * 7 + 2M) / 8 = 1.125M
        assert_eq!(state.bandwidth(), 1_125_000);
    }
}
