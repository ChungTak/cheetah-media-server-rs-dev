//! HLS player state machine for pull scenarios.
//!
//! HLS 拉流场景播放器状态机。
//! 管理播放列表刷新时机、分片去重与自适应码率选择。

use crate::parser::{ParsedMasterPlaylist, ParsedMediaPlaylist, ParsedVariant};

/// Adaptive bitrate selection strategy.
///
/// 自适应码率选择策略。
#[derive(Debug, Clone, Copy)]
pub enum BandwidthStrategy {
    /// Always pick the highest bandwidth variant.
    ///
    /// 始终选择最高码率变体。
    Highest,
    /// Always pick the lowest bandwidth variant.
    ///
    /// 始终选择最低码率变体。
    Lowest,
    /// Pick the highest variant whose bandwidth is within a safety factor of the estimate.
    ///
    /// 选择码率不超过估计带宽乘以安全系数（safety factor）的最高变体。
    Auto { safety_factor: f64 },
}

/// HLS player state — tracks playlist refresh, segment dedup, and ABR.
///
/// HLS 播放器状态 — 跟踪播放列表刷新、分片去重与 ABR。
///
/// Maintains the selected variant, media sequence cursor, a URI dedup set, and an
/// EWMA bandwidth estimate. The public fields are updated by the driver as the player
/// consumes playlists.
///
/// 维护已选变体、媒体序列游标、URI 去重集合与 EWMA 带宽估计。
/// 公共字段由驱动层在播放器消费播放列表时更新。
pub struct HlsPlayerState {
    /// Selected variant URI (from master playlist).
    ///
    /// 已选变体 URI（来自主播放列表）。
    pub selected_variant_uri: Option<String>,
    /// Last seen media sequence number.
    ///
    /// 上次看到的媒体序列号。
    pub last_sequence: i64,
    /// Set of segment URIs already downloaded (dedup).
    ///
    /// 已下载分片 URI 的集合（去重）。
    seen_uris: std::collections::HashSet<String>,
    /// Estimated bandwidth in bytes/sec (EWMA).
    ///
    /// 估计带宽（字节/秒，EWMA）。
    estimated_bandwidth: u64,
    /// Target duration from playlist (ms).
    ///
    /// 播放列表目标时长（毫秒）。
    pub target_duration_ms: u64,
    /// Consecutive unchanged playlist reloads.
    ///
    /// 连续未变化的播放列表重载次数。
    unchanged_count: u8,
    /// Whether the stream is live.
    ///
    /// 是否为直播流。
    pub is_live: bool,
}

impl HlsPlayerState {
    /// Create a new player state with default live assumptions.
    ///
    /// 创建带有默认直播假设的新播放器状态。
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
    ///
    /// - `Highest` picks the maximum bandwidth variant.
    /// - `Lowest` picks the minimum.
    /// - `Auto` filters variants below `estimated_bandwidth * safety_factor` and picks the
    ///   highest among those, falling back to the lowest if none qualify.
    ///
    /// 根据带宽策略从主播放列表中选择变体。
    /// - `Highest` 选择最高码率变体。
    /// - `Lowest` 选择最低码率变体。
    /// - `Auto` 过滤掉码率超过 `estimated_bandwidth * safety_factor` 的变体，
    ///   在符合条件的变体中选择最高码率；若无一符合则回退到最低码率。
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
    ///
    /// Updates `target_duration_ms` and `is_live`, compares the media sequence to the
    /// previous one, and deduplicates segments by URI (ignoring query parameters). If the
    /// seen set grows beyond 1024 entries, it is rebuilt from the current playlist to bound
    /// memory usage.
    ///
    /// 处理新获取的媒体播放列表，返回需要下载的新分片 URI。
    /// 更新 `target_duration_ms` 与 `is_live`；比较媒体序列号；按 URI 去重（忽略查询参数）。
    /// 若 seen 集合超过 1024 条，则从当前播放列表重建以限制内存。
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
    ///
    /// VOD streams need no refresh. For live, the delay is half the target duration after a
    /// changed playlist, and it backs off by `unchanged_count + 1` up to a multiplier of 5
    /// when the playlist is unchanged.
    ///
    /// 计算下一次播放列表刷新前的延迟（毫秒）。
    /// VOD 不需要刷新。直播中，播放列表变化后延迟为 `target_duration_ms / 2`；
    /// 未变化时按 `unchanged_count + 1` 退避，最大乘数为 5。
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
    ///
    /// Computes the instantaneous byte rate and updates the EWMA with a 7/8 weight on
    /// history and 1/8 on the new sample.
    ///
    /// 分片下载后更新带宽估计。
    /// 计算瞬时字节率，并以历史 7/8、新采样 1/8 的权重更新 EWMA。
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
    ///
    /// 当前估计带宽（字节/秒）。
    pub fn bandwidth(&self) -> u64 {
        self.estimated_bandwidth
    }

    /// Number of consecutive unchanged playlist reloads.
    ///
    /// 连续未变化的播放列表重载次数。
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
