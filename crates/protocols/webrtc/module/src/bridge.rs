//! Bridge between the WebRTC driver and the engine.
//!
//! Two flows live here:
//!
//! * **Ingress (publish)** — for sessions in role `Publisher` we acquire a
//!   `PublisherSink` from the engine and feed `WebRtcMediaEvent::Frame`
//!   events into it as `AVFrame`s. Track metadata is published lazily on
//!   the first frame for each MID. A simulcast policy filters frames so
//!   only one elected RID per MID reaches the engine.
//! * **Egress (play)** — for sessions in role `Player` we subscribe to the
//!   engine via [`spawn_play_subscriber`] and forward each frame back into
//!   the WebRTC driver as `WebRtcDriverCommand::SendFrame`. The driver
//!   then turns it into a `Writer::write` call inside `cheetah-webrtc-core`.

use std::collections::HashMap;
use std::sync::Arc;

use cheetah_codec::{
    AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat, FrameSideData, MediaKind, MonoTime,
    Timebase, TrackId, TrackInfo,
};
use cheetah_sdk::{
    PublishLease, PublisherApi, PublisherOptions, PublisherSink, RuntimeApi, SdkError, StreamKey,
};
use cheetah_webrtc_core::{MidLabel, WebRtcCodecKind, WebRtcMediaEvent, WebRtcSessionId};
use futures::FutureExt;
use parking_lot::Mutex;
use tracing::{debug, warn};

/// Snapshot of the currently elected simulcast rendition for a single MID.
///
/// 单个 MID 当前选中的 simulcast  rendition 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcRenditionSnapshot {
    pub mid: String,
    pub current_rid: Option<String>,
    pub seen_rids: Vec<String>,
}

/// Audio output policy for a play subscriber.
/// Carries the configured codec profile and audio output strategy so the play path can decide whether to pass through or transcode to Opus.
///
/// 播放订阅者的音频输出策略。
/// 携带配置的编解码器配置与音频输出策略，使播放路径可决定直通或转码为 Opus。
#[derive(Debug, Clone, Copy)]
pub struct PlaybackAudioPolicy {
    pub profile: crate::config::CodecProfileWire,
    pub strategy: crate::codec_policy::AudioOutputStrategy,
}

/// Playout timing policy for a play subscriber.
/// Combines jitter-buffer target and playout-delay bounds to compute the effective smoothing delay.
///
/// 播放订阅者的播放时序策略。
/// 结合抖动缓冲目标与播放延迟上下界，计算有效平滑延迟。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlaybackTimingPolicy {
    pub jitter_buffer_ms: u64,
    pub playout_delay_min_ms: u16,
    pub playout_delay_max_ms: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlaybackCodecMapping {
    pub codec: WebRtcCodecKind,
    pub clock_rate: u32,
}

/// Engine ingress state for a WebRTC publish session.
/// Acquires a PublisherSink, converts WebRTC media events into AVFrames, applies simulcast layer selection, and handles MultiStream sub-stream sinks.
///
/// WebRTC 发布会话的引擎入口状态。
/// 获取 PublisherSink，将 WebRTC 媒体事件转换为 AVFrame，应用 simulcast 层选择，并处理多流子流 sink。
pub struct WebRtcPublishBridge {
    stream_key: StreamKey,
    lease: PublishLease,
    sink: Box<dyn PublisherSink>,
    track_meta: HashMap<MidLabel, TrackMeta>,
    track_timestamp_epoch: HashMap<MidLabel, u32>,
    next_track_id: u32,
    simulcast: SimulcastSelection,
    rtcp_based_timestamp: bool,
    /// Set to `true` when the adaptive simulcast policy upgrades to a
    /// higher layer. The module event worker should consume this flag
    /// and request a PLI so the new layer starts with a keyframe.
    layer_upgrade_pending: bool,
    /// Per-RID sub-stream sinks for `SimulcastPolicy::MultiStream`.
    /// Lazily populated on first frame for each RID. The key is the
    /// RID string (e.g. "q", "h", "f").
    multistream_sinks: HashMap<String, (StreamKey, PublishLease, Box<dyn PublisherSink>)>,
    /// RIDs currently being acquired by an async task. Prevents
    /// duplicate `acquire_publisher` calls while the first call is
    /// still in flight.
    multistream_inflight: std::collections::HashSet<String>,
    /// Publisher API reference for lazy sub-stream acquisition in
    /// MultiStream mode. `None` for non-MultiStream policies.
    publisher_api: Option<Arc<dyn PublisherApi>>,
}

#[derive(Debug, Default)]
struct SimulcastSelection {
    /// Per-MID, the RID currently elected as the active layer. Other
    /// RIDs are dropped before reaching the engine.
    active_per_mid: HashMap<MidLabel, String>,
    /// Per-MID, all RIDs we have seen so far. Used to decide which
    /// layer the policy elects.
    seen_per_mid: HashMap<MidLabel, Vec<String>>,
    policy: crate::config::SimulcastPolicy,
    /// Latest BWE estimate (bps) for the session, fed in by the
    /// driver event worker. Only consulted when the policy is
    /// `Adaptive`. `None` means "no estimate yet" — we fall back to
    /// `Highest` until the first `WebRtcCoreEvent::Bwe` arrives.
    bwe_estimate_bps: Option<u64>,
    /// Latest REMB cap (bps) reported by the remote receiver. When
    /// both this and `bwe_estimate_bps` are set, layer election uses
    /// `min(remb, bwe)` so a REMB tighter than TWCC actually pulls
    /// down the elected layer rather than being silently overridden
    /// by the higher local estimate.
    remb_cap_bps: Option<u64>,
    /// `(low_bps, high_bps)` thresholds that bin the BWE estimate
    /// into low / mid / high. `0` on either side disables that bound.
    bwe_thresholds_bps: (u64, u64),
    /// NACK storm detector state.
    ///
    /// `nack_in` is "remote NACK requests received by us" — a sudden
    /// burst means the receiver is dropping enough packets to need
    /// retransmission, which is a reliable indicator of network
    /// congestion regardless of what TWCC / REMB are reporting. When
    /// the rate exceeds `nack_storm_threshold_per_sample`, the
    /// adaptive policy collapses to the lowest layer for one
    /// `nack_storm_recovery_samples` window.
    last_nack_in: u64,
    /// How many consecutive samples since we last detected a storm.
    /// `0` = currently in storm, increments back up to
    /// `nack_storm_recovery_samples` once the rate drops.
    nack_storm_recovery_left: u32,
    nack_storm_threshold_per_sample: u32,
    nack_storm_recovery_samples: u32,
}

impl SimulcastSelection {
    fn new(policy: crate::config::SimulcastPolicy, bwe_thresholds_bps: (u64, u64)) -> Self {
        Self {
            active_per_mid: HashMap::new(),
            seen_per_mid: HashMap::new(),
            policy,
            bwe_estimate_bps: None,
            remb_cap_bps: None,
            bwe_thresholds_bps,
            last_nack_in: 0,
            nack_storm_recovery_left: 0,
            nack_storm_threshold_per_sample: DEFAULT_NACK_STORM_THRESHOLD,
            nack_storm_recovery_samples: DEFAULT_NACK_STORM_RECOVERY_SAMPLES,
        }
    }

    fn set_bwe_estimate(&mut self, estimate_bps: u64) {
        self.bwe_estimate_bps = Some(estimate_bps);
    }

    fn set_remb_cap(&mut self, cap_bps: u64) {
        self.remb_cap_bps = Some(cap_bps);
    }

    /// Record the latest NACK-in counter (cumulative). Returns true
    /// when a storm was detected on this sample so the caller can
    /// surface a diagnostic for operators.
    fn observe_nack_in(&mut self, nack_in: u64) -> bool {
        let delta = nack_in.saturating_sub(self.last_nack_in);
        self.last_nack_in = nack_in;
        if delta >= u64::from(self.nack_storm_threshold_per_sample) {
            // Reset the recovery window: we'll stay pinned to
            // "lowest" for the next N samples regardless of the
            // BWE / REMB estimate.
            self.nack_storm_recovery_left = self.nack_storm_recovery_samples;
            true
        } else if self.nack_storm_recovery_left > 0 {
            // Decay the storm flag once each sample so we eventually
            // re-enter normal layer selection.
            self.nack_storm_recovery_left -= 1;
            false
        } else {
            false
        }
    }

    /// Effective rate cap fed into the adaptive layer-selection
    /// algorithm. When both BWE and REMB are present, the tighter
    /// constraint wins.
    fn effective_cap_bps(&self) -> Option<u64> {
        match (self.bwe_estimate_bps, self.remb_cap_bps) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    /// True when the NACK storm detector is still inside its
    /// recovery window. Layer election should treat this as a
    /// "force lowest" override.
    fn in_nack_storm(&self) -> bool {
        self.nack_storm_recovery_left > 0
    }

    /// Decide whether a frame on a given (MID, RID) should be admitted.
    ///
    /// `None` for RID means the publisher is not using simulcast and
    /// the frame is always admitted.
    ///
    /// Returns `(admitted, layer_upgraded)` where `layer_upgraded` is
    /// true when the elected layer just switched to a higher RID than
    /// the previous one. The caller should request a PLI/keyframe when
    /// this happens so the new layer starts with a decodable frame.
    fn admit_with_upgrade(&mut self, mid: &MidLabel, rid: Option<&str>) -> (bool, bool) {
        let rid = match rid {
            Some(r) if !r.is_empty() => r,
            _ => return (true, false),
        };
        // MultiStream mode: admit ALL layers — each RID will be
        // published as a separate sub-stream by the bridge.
        if matches!(self.policy, crate::config::SimulcastPolicy::MultiStream) {
            let seen = self.seen_per_mid.entry(mid.clone()).or_default();
            if !seen.iter().any(|s| s == rid) {
                seen.push(rid.to_string());
            }
            return (true, false);
        }
        // Snapshot the effective cap before borrowing `seen` mutably
        let cap = self.effective_cap_bps();
        let force_lowest = self.in_nack_storm();
        let seen = self.seen_per_mid.entry(mid.clone()).or_default();
        if !seen.iter().any(|s| s == rid) {
            seen.push(rid.to_string());
        }
        // Re-elect on each frame; this is cheap and lets new layers be
        // picked up automatically.
        let elected = if force_lowest {
            seen.iter()
                .min_by(|a, b| compare_rid_quality(a, b))
                .cloned()
        } else {
            elect_rid(seen, &self.policy, cap, self.bwe_thresholds_bps)
        };
        if let Some(elected) = elected {
            let previous = self.active_per_mid.get(mid).cloned();
            let upgraded = matches!(
                &previous,
                Some(prev) if prev != &elected && compare_rid_quality(&elected, prev).is_gt()
            );
            self.active_per_mid.insert(mid.clone(), elected.clone());
            (elected == rid, upgraded)
        } else {
            (false, false)
        }
    }

    /// Simplified admit that only returns whether the frame is admitted.
    #[cfg(test)]
    fn admit(&mut self, mid: &MidLabel, rid: Option<&str>) -> bool {
        self.admit_with_upgrade(mid, rid).0
    }

    fn rendition_snapshot(&self) -> Vec<WebRtcRenditionSnapshot> {
        let mut out: Vec<_> = self
            .seen_per_mid
            .iter()
            .map(|(mid, seen)| {
                let mut seen_rids = seen.clone();
                seen_rids.sort_by(|a, b| compare_rid_quality(a, b));
                WebRtcRenditionSnapshot {
                    mid: mid.as_str().to_string(),
                    current_rid: self.active_per_mid.get(mid).cloned(),
                    seen_rids,
                }
            })
            .collect();
        out.sort_by(|a, b| a.mid.cmp(&b.mid));
        out
    }
}

/// Default delta (samples emit once per second by str0m) above which
/// we declare a NACK storm. 50 NACKs in one second indicates serious
/// loss; lower thresholds cause spurious downgrades on Wi-Fi flutter.
const DEFAULT_NACK_STORM_THRESHOLD: u32 = 50;

/// Default number of subsequent samples we stay locked on the lowest
/// layer after a storm. With 1 s sample interval this is ~5 s of
/// hysteresis, matching SMS / ZLM "stay-on-low" behaviour after a
/// burst.
const DEFAULT_NACK_STORM_RECOVERY_SAMPLES: u32 = 5;

fn elect_rid(
    seen: &[String],
    policy: &crate::config::SimulcastPolicy,
    effective_cap_bps: Option<u64>,
    bwe_thresholds_bps: (u64, u64),
) -> Option<String> {
    use crate::config::SimulcastPolicy;
    match policy {
        SimulcastPolicy::Highest => seen
            .iter()
            .max_by(|a, b| compare_rid_quality(a, b))
            .cloned(),
        SimulcastPolicy::Lowest => seen
            .iter()
            .min_by(|a, b| compare_rid_quality(a, b))
            .cloned(),
        SimulcastPolicy::Rid(name) => seen.iter().find(|r| r == &name).cloned(),
        SimulcastPolicy::Adaptive => elect_adaptive(seen, effective_cap_bps, bwe_thresholds_bps),
        // MultiStream admits ALL layers — each RID is published as a
        // separate sub-stream. The `admit` logic returns `None` here
        // which signals the caller to use the multi-stream path
        // instead of the single-elected-RID path.
        SimulcastPolicy::MultiStream => None,
    }
}

fn compare_rid_quality(left: &str, right: &str) -> std::cmp::Ordering {
    match (known_rid_quality_rank(left), known_rid_quality_rank(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn known_rid_quality_rank(rid: &str) -> Option<u8> {
    match rid.trim().to_ascii_lowercase().as_str() {
        "q" | "quarter" | "low" | "l" => Some(0),
        "h" | "half" | "mid" | "medium" | "m" => Some(1),
        "f" | "full" | "high" => Some(2),
        _ => None,
    }
}

/// Map a BWE / REMB rate cap onto one of the seen RIDs.
///
/// Strategy (matches SMS / ZLM convention where lexicographic order
/// corresponds to ascending quality, e.g. `q < h < f`):
///
/// * No cap yet → fall back to `Highest`.
/// * Cap below `low_threshold` (when set) → pick the lowest RID.
/// * Cap above `high_threshold` (when set) → pick the highest.
/// * Otherwise → bin the seen RIDs into thirds and pick the middle.
///
/// For two-layer publishers, "middle" collapses onto the lower one
/// (matching the real-world expectation that a flaky estimate should
/// degrade rather than burst).
fn elect_adaptive(
    seen: &[String],
    effective_cap_bps: Option<u64>,
    bwe_thresholds_bps: (u64, u64),
) -> Option<String> {
    if seen.is_empty() {
        return None;
    }
    let mut sorted: Vec<&String> = seen.iter().collect();
    sorted.sort_by(|a, b| compare_rid_quality(a, b));

    let (low_bps, high_bps) = bwe_thresholds_bps;
    let cap = match effective_cap_bps {
        Some(v) => v,
        None => return sorted.last().cloned().cloned(),
    };

    if low_bps != 0 && cap < low_bps {
        return sorted.first().cloned().cloned();
    }
    if high_bps != 0 && cap > high_bps {
        return sorted.last().cloned().cloned();
    }
    // Mid range: pick the middle layer for ≥3 layers, else lowest.
    if sorted.len() >= 3 {
        let middle = sorted.len() / 2;
        sorted.get(middle).cloned().cloned()
    } else {
        sorted.first().cloned().cloned()
    }
}

#[derive(Debug, Clone, Copy)]
struct TrackMeta {
    track_id: TrackId,
    codec: CodecId,
    media_kind: MediaKind,
    clock_rate: u32,
}

impl WebRtcPublishBridge {
    /// Acquire a publisher lease and create a new publish bridge.
    /// Initializes simulcast selection state and, for MultiStream mode, keeps a reference to the publisher API so per-RID sub-stream sinks can be acquired lazily.
    ///
    /// 获取发布租约并创建新的发布桥。
    /// 初始化 simulcast 选择状态；在多流模式下保留 publisher API 引用，以便按需获取每个 RID 的子流 sink。
    pub async fn acquire(
        publisher_api: &Arc<dyn PublisherApi>,
        stream_key: StreamKey,
        simulcast_policy: crate::config::SimulcastPolicy,
        bwe_thresholds_bps: (u64, u64),
        rtcp_based_timestamp: bool,
    ) -> Result<Self, SdkError> {
        let (lease, sink) = publisher_api
            .acquire_publisher(stream_key.clone(), PublisherOptions::default())
            .await?;
        let is_multistream = matches!(
            simulcast_policy,
            crate::config::SimulcastPolicy::MultiStream
        );
        Ok(Self {
            stream_key,
            lease,
            sink,
            track_meta: HashMap::new(),
            track_timestamp_epoch: HashMap::new(),
            next_track_id: 1,
            simulcast: SimulcastSelection::new(simulcast_policy, bwe_thresholds_bps),
            rtcp_based_timestamp,
            layer_upgrade_pending: false,
            multistream_sinks: HashMap::new(),
            multistream_inflight: std::collections::HashSet::new(),
            publisher_api: if is_multistream {
                Some(publisher_api.clone())
            } else {
                None
            },
        })
    }

    pub fn stream_key(&self) -> &StreamKey {
        &self.stream_key
    }

    pub fn lease(&self) -> &PublishLease {
        &self.lease
    }

    /// Update the BWE estimate used for adaptive simulcast layer selection.
    ///
    /// 更新用于自适应 simulcast 层选择的 BWE 估计值。
    pub fn set_bwe_estimate(&mut self, estimate_bps: u64) {
        self.simulcast.set_bwe_estimate(estimate_bps);
    }

    /// Update the REMB-driven bitrate cap used for adaptive simulcast layer selection.
    /// When both BWE and REMB are present the policy uses the lower of the two, preventing overshoot of the receiver-suggested ceiling.
    ///
    /// 更新用于自适应 simulcast 层选择的 REMB 驱动码率上限。
    /// 当 BWE 与 REMB 同时存在时，策略取二者较小值，防止超出接收端建议上限。
    pub fn set_remb_cap(&mut self, cap_bps: u64) {
        self.simulcast.set_remb_cap(cap_bps);
    }

    /// Feed an egress NACK counter into the bridge's NACK-storm detector.
    /// Returns true when the storm threshold is tripped, causing the adaptive simulcast policy to pin to the lowest layer for a recovery window.
    ///
    /// 将出口 NACK 计数器送入桥的 NACK 风暴检测器。
    /// 当触发风暴阈值时返回 true，使自适应 simulcast 策略在恢复窗口内固定到最低层。
    pub fn observe_nack_in(&mut self, nack_in: u64) -> bool {
        self.simulcast.observe_nack_in(nack_in)
    }

    /// Return and clear the pending layer-upgrade flag.
    /// The caller should request a keyframe so the newly elected higher layer starts with a decodable frame.
    ///
    /// 返回并清除待处理的层升级标志。
    /// 调用方应请求关键帧，使新选中的更高层从可解码帧开始。
    pub fn take_layer_upgrade_pending(&mut self) -> bool {
        let pending = self.layer_upgrade_pending;
        self.layer_upgrade_pending = false;
        pending
    }

    /// Return a snapshot of currently elected simulcast rids per MID.
    ///
    /// 返回每个 MID 当前选中的 simulcast rid 快照。
    pub fn rendition_snapshot(&self) -> Vec<WebRtcRenditionSnapshot> {
        self.simulcast.rendition_snapshot()
    }

    /// Push a WebRTC media event into the engine as an AVFrame.
    /// Applies simulcast admission, converts RTP timestamp to a canonical microsecond timeline, maps codec kind to CodecId, updates track metadata, and routes to the primary or per-RID sub-stream sink.
    ///
    /// 将 WebRTC 媒体事件作为 AVFrame 推入引擎。
    /// 应用 simulcast 准入，将 RTP 时间戳转换为规范微秒时间线，映射编解码器类型到 CodecId，更新 track 元数据，并路由到主 sink 或按 RID 子流 sink。
    pub fn push_frame(&mut self, event: WebRtcMediaEvent) {
        let WebRtcMediaEvent::Frame {
            mid,
            rid,
            codec,
            clock_rate,
            random_access,
            rtp_timestamp_ticks,
            rtp_timestamp_denom,
            payload,
            network_time_micros,
            meta: frame_meta,
        } = event
        else {
            return;
        };

        if payload.is_empty() {
            return;
        }
        if clock_rate == 0 {
            return;
        }

        // Apply simulcast layer policy: drop frames from non-elected RIDs.
        // When the elected layer upgrades (switches to a higher RID),
        // record that a PLI should be requested so the new layer starts
        // with a decodable keyframe.
        let (admitted, layer_upgraded) = self.simulcast.admit_with_upgrade(&mid, rid.as_deref());
        if layer_upgraded {
            self.layer_upgrade_pending = true;
        }
        if !admitted {
            return;
        }

        let codec_id = match map_codec(codec) {
            Some(c) => c,
            None => {
                debug!("dropping WebRTC frame with unmapped codec {codec:?}");
                return;
            }
        };

        let media_kind = if codec_id_is_video(codec_id) {
            MediaKind::Video
        } else {
            MediaKind::Audio
        };

        let meta = match self.track_meta.get(&mid).copied() {
            Some(m) if m.codec == codec_id && m.clock_rate == clock_rate => m,
            // Track exists but codec/clock-rate changed (e.g.,
            // post-renegotiation), or we have not seen this mid before.
            // Either way we re-emit the full track list to the engine so
            // it sees a consistent set; passing only the changed track
            // would replace the engine's track snapshot and drop sibling
            // audio/video tracks (engine `update_tracks` is a full
            // replace, not a merge).
            Some(_) | None => {
                let track_id = match self.track_meta.get(&mid) {
                    Some(prev) => prev.track_id,
                    None => {
                        let id = TrackId(self.next_track_id);
                        self.next_track_id = self.next_track_id.saturating_add(1);
                        id
                    }
                };
                let mut info = TrackInfo::new(track_id, media_kind, codec_id, clock_rate);
                if matches!(codec_id, CodecId::H264) {
                    info.extradata = CodecExtradata::H264 {
                        sps: Vec::new(),
                        pps: Vec::new(),
                        avcc: None,
                    };
                }
                info.refresh_readiness();
                let new_meta = TrackMeta {
                    track_id,
                    codec: codec_id,
                    media_kind,
                    clock_rate,
                };
                // Stage the new meta locally before computing the union
                // so the rebuilt `tracks` snapshot already reflects this
                // change (including media_kind / codec switches for the
                // same mid).
                self.track_meta.insert(mid.clone(), new_meta);
                let tracks = self.build_tracks_snapshot(&mid, info);
                if let Err(err) = self.sink.update_tracks(tracks) {
                    warn!(
                        "WebRTC publish update_tracks failed for {}: {err}",
                        self.stream_key
                    );
                }
                new_meta
            }
        };

        // Convert the WebRTC RTP timestamp into a canonical microsecond
        // timeline. We use the stream clock rate as the timebase
        // denominator, which is what `cheetah-codec` egress helpers
        // expect for downstream RTP encapsulation.
        let denom = if rtp_timestamp_denom != 0 {
            rtp_timestamp_denom
        } else {
            meta.clock_rate
        };
        let timebase = Timebase::new(1, denom);
        let timestamp_ticks = if self.rtcp_based_timestamp {
            rtp_timestamp_ticks
        } else {
            let epoch = self
                .track_timestamp_epoch
                .entry(mid.clone())
                .or_insert(rtp_timestamp_ticks);
            rtp_timestamp_ticks.wrapping_sub(*epoch)
        };
        let pts = timestamp_ticks as i64;
        let dts = pts;
        let format = match meta.codec {
            CodecId::H264 | CodecId::H265 | CodecId::H266 => FrameFormat::CanonicalH26x,
            CodecId::Opus => FrameFormat::OpusPacket,
            CodecId::G711A | CodecId::G711U => FrameFormat::G711Packet,
            CodecId::AAC => FrameFormat::AacRaw,
            CodecId::AV1 => FrameFormat::CanonicalAv1Obu,
            CodecId::VP8 => FrameFormat::CanonicalVp8Frame,
            CodecId::VP9 => FrameFormat::CanonicalVp9Frame,
            _ => FrameFormat::Unknown,
        };
        let mut frame = AVFrame::new(
            meta.track_id,
            meta.media_kind,
            meta.codec,
            format,
            pts,
            dts,
            timebase,
            payload,
        );
        if random_access {
            frame.flags.insert(FrameFlags::KEY);
        }
        // Surface RTP-level discontinuities reported by str0m's
        // reorder buffer so the timestamp normalizer / downstream
        // codec layer can mark the gap. We only record the flag on
        // the first frame after a gap; subsequent contiguous frames
        // clear it implicitly because each `AVFrame` starts with an
        // empty `flags` set.
        if !frame_meta.contiguous {
            frame.flags.insert(FrameFlags::DISCONTINUITY);
        }
        // Forward the RTP sequence number (first packet that
        // contributed to this access unit) as canonical side data.
        // Codec-level adapters consume this when building the
        // `WebRtcIngressContractView` for downstream egress.
        if let Some(seq) = frame_meta.sequence_number {
            frame
                .side_data
                .push(FrameSideData::SequenceNumber(u64::from(seq)));
        }
        // Audio-level / voice-activity / video-orientation get
        // surfaced as opaque key/value metadata so codec adapters
        // that care can pick them up without baking RTP-extension
        // semantics into the canonical AVFrame shape. We use a
        // stable `webrtc.*` key prefix matching ZLM / SMS
        // conventions.
        if let Some(level) = frame_meta.audio_level_dbov {
            frame.side_data.push(FrameSideData::Metadata {
                key: "webrtc.audio_level_dbov".into(),
                value: level.to_string(),
            });
        }
        if let Some(va) = frame_meta.voice_activity {
            frame.side_data.push(FrameSideData::Metadata {
                key: "webrtc.voice_activity".into(),
                value: if va { "1" } else { "0" }.into(),
            });
        }
        if let Some(orient) = frame_meta.video_orientation {
            frame.side_data.push(FrameSideData::Metadata {
                key: "webrtc.video_orientation".into(),
                value: orient.to_string(),
            });
        }
        // The driver injects network time as microseconds since the core
        // anchor. We surface that into the frame's PTS-microsecond field
        // for engine ring buffer ordering. `denom` is non-zero by the
        // checks above (`clock_rate == 0` early-returns and
        // `meta.clock_rate` is captured from a non-zero `clock_rate`).
        let pts_us = ((timestamp_ticks as i64) * 1_000_000) / denom as i64;
        frame.pts_us = pts_us;
        frame.dts_us = pts_us;
        let _ = network_time_micros;

        if let Err(err) = self.push_to_sink(rid.as_deref(), Arc::new(frame)) {
            debug!(
                "WebRTC publish push_frame to {} failed: {err}",
                self.stream_key
            );
        }
    }

    /// Route a frame to the appropriate sink. In MultiStream mode,
    /// frames with a RID are routed to the per-RID sub-stream sink
    /// (lazily acquired). Frames without a RID or in non-MultiStream
    /// mode go to the primary sink.
    fn push_to_sink(&mut self, rid: Option<&str>, frame: Arc<AVFrame>) -> Result<(), SdkError> {
        let is_multistream = matches!(
            self.simulcast.policy,
            crate::config::SimulcastPolicy::MultiStream
        );
        if is_multistream {
            if let Some(rid_str) = rid {
                if !rid_str.is_empty() {
                    // Route to per-RID sub-stream sink
                    if let Some((_key, _lease, sink)) = self.multistream_sinks.get(rid_str) {
                        sink.push_frame(frame).map(|_| ())
                    } else {
                        // Sub-stream not yet acquired — fall through to
                        // primary sink so frames are not lost. The module
                        // event worker will acquire it via
                        // `acquire_multistream_sink` on the next tick.
                        self.sink.push_frame(frame).map(|_| ())
                    }
                } else {
                    self.sink.push_frame(frame).map(|_| ())
                }
            } else {
                self.sink.push_frame(frame).map(|_| ())
            }
        } else {
            self.sink.push_frame(frame).map(|_| ())
        }
    }

    /// Return RIDs that have been seen but do not yet have sub-stream sinks in MultiStream mode.
    /// Skips RIDs that are already in flight so async acquisition does not duplicate work.
    ///
    /// 返回在多流模式下已出现但尚无子流 sink 的 RID 列表。
    /// 跳过已在进行中的 RID，避免异步获取重复工作。
    pub fn pending_multistream_rids(&self) -> Vec<String> {
        if !matches!(
            self.simulcast.policy,
            crate::config::SimulcastPolicy::MultiStream
        ) {
            return Vec::new();
        }
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for rids in self.simulcast.seen_per_mid.values() {
            for rid in rids {
                if !self.multistream_sinks.contains_key(rid.as_str())
                    && !self.multistream_inflight.contains(rid.as_str())
                {
                    seen.insert(rid.clone());
                }
            }
        }
        seen.into_iter().collect()
    }

    /// Mark a set of RIDs as having an in-flight acquire_publisher task.
    ///
    /// 将一组 RID 标记为正在进行 acquire_publisher 任务。
    pub fn mark_multistream_inflight(&mut self, rids: &[String]) {
        for rid in rids {
            self.multistream_inflight.insert(rid.clone());
        }
    }

    /// Clear the in-flight marker for a RID without inserting a sink, allowing the RID to become pending again on the next frame.
    ///
    /// 清除某个 RID 的进行中标记而不插入 sink，使该 RID 在下一帧再次变为待处理。
    pub fn clear_multistream_inflight(&mut self, rid: &str) {
        self.multistream_inflight.remove(rid);
    }

    /// Get the publisher API and base stream key for async sub-stream acquisition.
    ///
    /// 获取异步子流获取所需的 publisher API 与基础 stream key。
    pub fn publisher_api_and_stream_key(&self) -> Option<(Arc<dyn PublisherApi>, StreamKey)> {
        self.publisher_api
            .as_ref()
            .map(|api| (api.clone(), self.stream_key.clone()))
    }

    /// Insert a pre-acquired sub-stream sink for a specific RID and clear the in-flight marker.
    ///
    /// 为指定 RID 插入预获取的子流 sink 并清除进行中标记。
    pub fn insert_multistream_sink(
        &mut self,
        rid: String,
        key: StreamKey,
        lease: PublishLease,
        sink: Box<dyn PublisherSink>,
    ) {
        self.multistream_inflight.remove(&rid);
        self.multistream_sinks.insert(rid, (key, lease, sink));
    }

    /// Close the bridge and all its MultiStream sub-stream sinks.
    ///
    /// 关闭桥及其所有多流子流 sink。
    pub fn close(&self) {
        let _ = self.sink.close();
        for (_key, _lease, sink) in self.multistream_sinks.values() {
            let _ = sink.close();
        }
    }

    /// Build the full TrackInfo snapshot to push into the engine.
    ///
    /// `mid` and `latest` describe the track currently being (re-)registered;
    /// the rest are reconstructed from `self.track_meta` so the engine sees
    /// a stable union after every codec / clock-rate change.
    fn build_tracks_snapshot(&self, mid: &MidLabel, latest: TrackInfo) -> Vec<TrackInfo> {
        let mut tracks = Vec::with_capacity(self.track_meta.len());
        for (existing_mid, meta) in &self.track_meta {
            if existing_mid == mid {
                continue;
            }
            let mut info =
                TrackInfo::new(meta.track_id, meta.media_kind, meta.codec, meta.clock_rate);
            if matches!(meta.codec, CodecId::H264) {
                info.extradata = CodecExtradata::H264 {
                    sps: Vec::new(),
                    pps: Vec::new(),
                    avcc: None,
                };
            }
            info.refresh_readiness();
            tracks.push(info);
        }
        tracks.push(latest);
        tracks
    }
}

/// Derive a sub-stream key for MultiStream simulcast mode.
/// Appends @rid:<name> to the base stream path so downstream subscribers can select individual layers.
///
/// 为多流 simulcast 模式派生子流 key。
/// 将 @rid:<name> 附加到基础流路径，使下游订阅者可选择独立层。
pub fn derive_multistream_key(base: &StreamKey, rid: &str) -> StreamKey {
    let path = format!("{}@rid:{}", base.path, rid);
    StreamKey::new(&base.namespace, path)
}

fn map_codec(codec: WebRtcCodecKind) -> Option<CodecId> {
    Some(match codec {
        WebRtcCodecKind::H264 => CodecId::H264,
        WebRtcCodecKind::H265 => CodecId::H265,
        WebRtcCodecKind::Vp8 => CodecId::VP8,
        WebRtcCodecKind::Vp9 => CodecId::VP9,
        WebRtcCodecKind::Av1 => CodecId::AV1,
        WebRtcCodecKind::Opus => CodecId::Opus,
        WebRtcCodecKind::Pcma => CodecId::G711A,
        WebRtcCodecKind::Pcmu => CodecId::G711U,
        WebRtcCodecKind::Unknown => return None,
    })
}

fn codec_id_is_video(codec: CodecId) -> bool {
    matches!(
        codec,
        CodecId::H264 | CodecId::H265 | CodecId::H266 | CodecId::VP8 | CodecId::VP9 | CodecId::AV1
    )
}

/// Per-session mapping of MIDs to audio and video tracks for play sessions.
///
/// 播放会话中 MID 到音频与视频 track 的映射。
#[derive(Debug, Default)]
pub struct PlayTrackMap {
    pub video_mid: Option<MidLabel>,
    pub audio_mid: Option<MidLabel>,
}

impl PlayTrackMap {
    /// Record a MID assignment for a given media kind.
    ///
    /// 记录指定媒体类型对应的 MID 分配。
    pub fn record(&mut self, mid: MidLabel, kind: cheetah_webrtc_core::WebRtcMediaKind) {
        match kind {
            cheetah_webrtc_core::WebRtcMediaKind::Audio => self.audio_mid = Some(mid),
            cheetah_webrtc_core::WebRtcMediaKind::Video => self.video_mid = Some(mid),
        }
    }
}

/// Registry of active publish bridges and play subscriber tokens, keyed by WebRTC session id.
///
/// 按 WebRTC 会话 id 索引的活跃发布桥与播放订阅者 token 注册表。
#[derive(Default)]
pub struct WebRtcBridgeRegistry {
    publish: HashMap<WebRtcSessionId, WebRtcPublishBridge>,
    play: HashMap<WebRtcSessionId, cheetah_sdk::CancellationToken>,
    play_tracks: HashMap<WebRtcSessionId, PlayTrackMap>,
    play_stats: HashMap<WebRtcSessionId, WebRtcPlayBootstrapStats>,
}

/// GOP bootstrap timing and frame counts for a player session.
/// Tracks how long it takes from subscriber start to first frame, first keyframe, and first decodable frame, plus playout delay and delayed-frame metrics.
///
/// 播放会话的 GOP 引导时序与帧计数。
/// 记录从订阅者启动到首帧、首个关键帧、首个可解码帧的耗时，以及播放延迟和延迟帧指标。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebRtcPlayBootstrapStats {
    /// Wall-clock micros from subscriber start to the first frame the
    /// subscriber forwarded to the driver.
    pub first_frame_micros: Option<u64>,
    /// Wall-clock micros to the first keyframe (random_access=true)
    /// the subscriber forwarded.
    pub first_keyframe_micros: Option<u64>,
    /// Wall-clock micros to the first frame whose codec config /
    /// extradata was either inline or provided as a parameter set
    /// (random_access frame for H264/H265). For audio-only streams
    /// this defaults to `first_frame_micros`.
    pub first_decodable_micros: Option<u64>,
    /// Total frames forwarded by the play subscriber.
    pub frames_forwarded: u64,
    /// Total keyframes forwarded.
    pub keyframes_forwarded: u64,
    /// If the play subscriber's slow-start wait window expired without
    /// the stream coming online, this records the elapsed wait in
    /// milliseconds. `None` means the stream was found within the
    /// window (or the subscriber has not yet timed out).
    pub wait_timeout_elapsed_ms: Option<u64>,
    /// Configured jitter buffer target in milliseconds.
    pub jitter_buffer_ms: u64,
    /// Configured playout-delay lower bound in milliseconds.
    pub playout_delay_min_ms: u16,
    /// Configured playout-delay upper bound in milliseconds.
    pub playout_delay_max_ms: u16,
    /// Effective target delay actually applied by the play sender.
    pub effective_playout_delay_ms: u64,
    /// Number of frames delayed by the jitter/playout smoothing path.
    pub delayed_frames: u64,
    /// Total sleep time injected by smoothing, in microseconds.
    pub delayed_total_micros: u64,
}

impl WebRtcBridgeRegistry {
    /// Insert a publish bridge for a session.
    ///
    /// 为会话插入发布桥。
    pub fn insert_publish(&mut self, session_id: WebRtcSessionId, bridge: WebRtcPublishBridge) {
        self.publish.insert(session_id, bridge);
    }

    /// Remove and return the publish bridge for a session.
    ///
    /// 移除并返回会话的发布桥。
    pub fn remove_publish(&mut self, session_id: WebRtcSessionId) -> Option<WebRtcPublishBridge> {
        self.publish.remove(&session_id)
    }

    /// Insert a cancellation token for a play subscriber.
    ///
    /// 为播放订阅者插入取消 token。
    pub fn insert_play(
        &mut self,
        session_id: WebRtcSessionId,
        cancel: cheetah_sdk::CancellationToken,
    ) {
        self.play.insert(session_id, cancel);
    }

    /// Remove the play subscriber token and its associated track and stats entries.
    ///
    /// 移除播放订阅者 token 及其关联的 track 和 stats 条目。
    pub fn remove_play(
        &mut self,
        session_id: WebRtcSessionId,
    ) -> Option<cheetah_sdk::CancellationToken> {
        self.play_tracks.remove(&session_id);
        self.play_stats.remove(&session_id);
        self.play.remove(&session_id)
    }

    /// Push a media event into the publish bridge for a session.
    ///
    /// 将媒体事件推送到会话的发布桥。
    pub fn push_publish_frame(
        &mut self,
        session_id: WebRtcSessionId,
        event: WebRtcMediaEvent,
    ) -> bool {
        if let Some(bridge) = self.publish.get_mut(&session_id) {
            bridge.push_frame(event);
            true
        } else {
            false
        }
    }

    /// Return and clear the pending layer-upgrade flag for a publish bridge.
    ///
    /// 返回并清除发布桥的待处理层升级标志。
    pub fn take_publish_layer_upgrade(&mut self, session_id: WebRtcSessionId) -> bool {
        self.publish
            .get_mut(&session_id)
            .map(|b| b.take_layer_upgrade_pending())
            .unwrap_or(false)
    }

    /// Check whether a publish bridge exists for a session.
    ///
    /// 检查会话是否存在发布桥。
    pub fn contains_publish(&self, session_id: WebRtcSessionId) -> bool {
        self.publish.contains_key(&session_id)
    }

    /// Get a mutable reference to the publish bridge for a session.
    /// Used by the module event worker for async MultiStream sink acquisition.
    ///
    /// 获取会话发布桥的可变引用。
    /// 模块事件工作线程用于异步多流 sink 获取。
    pub fn publish_mut(&mut self, session_id: WebRtcSessionId) -> Option<&mut WebRtcPublishBridge> {
        self.publish.get_mut(&session_id)
    }

    /// Check if the publish bridge for `session_id` has pending
    /// MultiStream RIDs that need sub-stream sinks acquired.
    pub fn pending_multistream_rids(&self, session_id: WebRtcSessionId) -> Vec<String> {
        self.publish
            .get(&session_id)
            .map(|b| b.pending_multistream_rids())
            .unwrap_or_default()
    }

    /// Thread a BWE estimate into the publish bridge for a session.
    ///
    /// 将 BWE 估计值传入会话的发布桥。
    pub fn set_publish_bwe_estimate(
        &mut self,
        session_id: WebRtcSessionId,
        estimate_bps: u64,
    ) -> bool {
        if let Some(bridge) = self.publish.get_mut(&session_id) {
            bridge.set_bwe_estimate(estimate_bps);
            true
        } else {
            false
        }
    }

    /// Thread a REMB cap into the publish bridge for a session.
    ///
    /// 将 REMB 上限传入会话的发布桥。
    pub fn set_publish_remb_cap(&mut self, session_id: WebRtcSessionId, cap_bps: u64) -> bool {
        if let Some(bridge) = self.publish.get_mut(&session_id) {
            bridge.set_remb_cap(cap_bps);
            true
        } else {
            false
        }
    }

    /// Feed an egress NACK counter into the publish bridge's storm detector.
    ///
    /// 将出口 NACK 计数器送入发布桥的风暴检测器。
    pub fn record_publish_nack_in(&mut self, session_id: WebRtcSessionId, nack_in: u64) -> bool {
        if let Some(bridge) = self.publish.get_mut(&session_id) {
            bridge.observe_nack_in(nack_in)
        } else {
            false
        }
    }

    /// Record a MID-to-kind mapping for a play session.
    ///
    /// 记录播放会话的 MID 到媒体类型映射。
    pub fn record_play_track(
        &mut self,
        session_id: WebRtcSessionId,
        mid: MidLabel,
        kind: cheetah_webrtc_core::WebRtcMediaKind,
    ) {
        self.play_tracks
            .entry(session_id)
            .or_default()
            .record(mid, kind);
    }

    /// Look up the MID for a given media kind in a play session.
    ///
    /// 在播放会话中查找指定媒体类型的 MID。
    pub fn play_track_for(
        &self,
        session_id: WebRtcSessionId,
        kind: cheetah_webrtc_core::WebRtcMediaKind,
    ) -> Option<MidLabel> {
        let map = self.play_tracks.get(&session_id)?;
        match kind {
            cheetah_webrtc_core::WebRtcMediaKind::Audio => map.audio_mid.clone(),
            cheetah_webrtc_core::WebRtcMediaKind::Video => map.video_mid.clone(),
        }
    }

    /// Record that a play subscriber forwarded a frame.
    /// Updates first-frame, first-keyframe, and first-decodable timing in microseconds since subscriber start.
    ///
    /// 记录播放订阅者转发了一帧。
    /// 更新自订阅者启动以来的首帧、首个关键帧、首个可解码帧的微秒级时序。
    pub fn record_play_frame(
        &mut self,
        session_id: WebRtcSessionId,
        now_micros: u64,
        random_access: bool,
        has_codec_config: bool,
    ) {
        let stats = self.play_stats.entry(session_id).or_default();
        stats.frames_forwarded = stats.frames_forwarded.saturating_add(1);
        if stats.first_frame_micros.is_none() {
            stats.first_frame_micros = Some(now_micros);
        }
        if random_access {
            stats.keyframes_forwarded = stats.keyframes_forwarded.saturating_add(1);
            if stats.first_keyframe_micros.is_none() {
                stats.first_keyframe_micros = Some(now_micros);
            }
        }
        if stats.first_decodable_micros.is_none() && (has_codec_config || random_access) {
            stats.first_decodable_micros = Some(now_micros);
        }
    }

    pub fn record_play_timing_policy(
        &mut self,
        session_id: WebRtcSessionId,
        policy: PlaybackTimingPolicy,
        effective_delay_ms: u64,
    ) {
        let stats = self.play_stats.entry(session_id).or_default();
        stats.jitter_buffer_ms = policy.jitter_buffer_ms;
        stats.playout_delay_min_ms = policy.playout_delay_min_ms;
        stats.playout_delay_max_ms = policy.playout_delay_max_ms;
        stats.effective_playout_delay_ms = effective_delay_ms;
    }

    pub fn record_play_timing_delay(&mut self, session_id: WebRtcSessionId, delayed_micros: u64) {
        if delayed_micros == 0 {
            return;
        }
        let stats = self.play_stats.entry(session_id).or_default();
        stats.delayed_frames = stats.delayed_frames.saturating_add(1);
        stats.delayed_total_micros = stats.delayed_total_micros.saturating_add(delayed_micros);
    }

    /// Snapshot current bootstrap stats for a player session. Returns
    /// `None` if the session has not forwarded any frames yet.
    pub fn play_stats(&self, session_id: WebRtcSessionId) -> Option<WebRtcPlayBootstrapStats> {
        self.play_stats.get(&session_id).cloned()
    }

    pub fn publish_renditions(
        &self,
        session_id: WebRtcSessionId,
    ) -> Option<Vec<WebRtcRenditionSnapshot>> {
        self.publish
            .get(&session_id)
            .map(|bridge| bridge.rendition_snapshot())
    }

    /// Record that the play subscriber's slow-start wait window expired
    /// without the stream coming online.
    pub fn record_play_timeout(&mut self, session_id: WebRtcSessionId, elapsed_ms: u64) {
        let stats = self.play_stats.entry(session_id).or_default();
        stats.wait_timeout_elapsed_ms = Some(elapsed_ms);
    }
}

/// Close all publish bridges and cancel all play subscribers on shutdown.
///
/// 关闭时关闭所有发布桥并取消所有播放订阅者。
pub fn close_all(registry: Arc<Mutex<WebRtcBridgeRegistry>>) {
    let mut guard = registry.lock();
    for (_, bridge) in guard.publish.drain() {
        bridge.close();
    }
    for (_, cancel) in guard.play.drain() {
        cancel.cancel();
    }
    guard.play_tracks.clear();
    guard.play_stats.clear();
}

#[derive(Debug)]
struct PlaybackTimingState {
    effective_delay: std::time::Duration,
    anchor_pts_us: Option<i64>,
    anchor_instant: Option<std::time::Instant>,
}

impl PlaybackTimingState {
    fn new(policy: PlaybackTimingPolicy) -> (Self, u64) {
        let mut effective_delay = policy
            .jitter_buffer_ms
            .max(policy.playout_delay_min_ms as u64);
        if policy.playout_delay_max_ms != 0 {
            effective_delay = effective_delay.min(policy.playout_delay_max_ms as u64);
        }
        (
            Self {
                effective_delay: std::time::Duration::from_millis(effective_delay),
                anchor_pts_us: None,
                anchor_instant: None,
            },
            effective_delay,
        )
    }

    async fn apply(&mut self, pts_us: i64, runtime: &Arc<dyn RuntimeApi>) -> u64 {
        if self.effective_delay.is_zero() {
            return 0;
        }
        let now = std::time::Instant::now();
        if self.anchor_pts_us.is_none() || self.anchor_instant.is_none() {
            self.anchor_pts_us = Some(pts_us);
            self.anchor_instant = Some(now);
        }
        let base_pts = self.anchor_pts_us.unwrap_or(pts_us);
        let base_instant = self.anchor_instant.unwrap_or(now);
        let delta_us = pts_us.saturating_sub(base_pts);
        // Large positive jumps usually indicate a discontinuity. Reset
        // the anchor to avoid sleeping for multi-second gaps.
        if delta_us > 2_000_000 {
            self.anchor_pts_us = Some(pts_us);
            self.anchor_instant = Some(now);
            sleep_for_duration(runtime, self.effective_delay).await;
            return self.effective_delay.as_micros() as u64;
        }
        let media_delta = if delta_us > 0 {
            std::time::Duration::from_micros(delta_us as u64)
        } else {
            std::time::Duration::ZERO
        };
        let target = base_instant + self.effective_delay + media_delta;
        if target <= now {
            return 0;
        }
        let sleep_for = target.duration_since(now);
        sleep_for_duration(runtime, sleep_for).await;
        sleep_for.as_micros() as u64
    }
}

/// Sleep for a relative duration using the injected runtime timer.
async fn sleep_for_duration(runtime: &Arc<dyn RuntimeApi>, dur: std::time::Duration) {
    let dur_us = u64::try_from(dur.as_micros()).unwrap_or(u64::MAX);
    let deadline = MonoTime::from_micros(runtime.now().as_micros().saturating_add(dur_us));
    runtime.sleep_until(deadline).wait().await;
}

/// Spawn an engine subscriber that forwards `AVFrame`s to a WebRTC
/// player session via the driver.
///
/// The subscriber stops as soon as the supplied cancellation token
/// fires or the engine subscriber's stream closes.
///
/// `wait_stream_timeout_ms` triggers a slow-start retry loop: when
/// the engine reports `SdkError::NotFound`, the subscriber retries
/// every 100 ms until the configured window elapses. Within the
/// window the subscribe is treated as eventually-consistent, matching
/// the SMS / ZLM behaviour where a player can join a stream that the
/// publisher is about to push. Outside the window the original
/// `NotFound` is returned to the caller.
///
/// ## Codec Bootstrap
///
/// The subscriber uses [`crate::bootstrap::PlayBootstrapView`] to
/// ensure new subscribers receive decodable keyframe sequences. The
/// view delegates all parameter set caching to `cheetah-codec`'s
/// `ParameterSetCache` — the WebRTC module does NOT maintain its own
/// private SPS/PPS/VPS map.
///
/// ## B-frame Filter
///
/// When `h264_bframe_filter` is true, H264 B-frames are dropped
/// before reaching the driver. This is independent of parameter set
/// prepend: the filter is an admission decision, while prepend
/// modifies admitted keyframe payloads.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_play_subscriber(
    ctx: cheetah_sdk::EngineContext,
    driver: Arc<cheetah_webrtc_driver_tokio::WebRtcDriverHandle>,
    bridges: Arc<Mutex<WebRtcBridgeRegistry>>,
    session_id: WebRtcSessionId,
    stream_key: cheetah_sdk::StreamKey,
    bootstrap_frames: usize,
    bootstrap_max_age_ms: u64,
    wait_stream_timeout_ms: u64,
    h264_bframe_filter: bool,
    audio_policy: PlaybackAudioPolicy,
    timing_policy: PlaybackTimingPolicy,
    cancel: cheetah_sdk::CancellationToken,
    start_instant: std::time::Instant,
) -> Result<(), SdkError> {
    use cheetah_codec::MediaKind;
    use cheetah_sdk::SubscriberOptions;
    let opts = SubscriberOptions {
        queue_capacity: 256,
        bootstrap_policy: cheetah_sdk::BootstrapPolicy::live_tail(
            bootstrap_frames,
            Some(bootstrap_max_age_ms),
        ),
        ..Default::default()
    };

    // Slow-start retry: WHEP / ZLM-style players may connect before
    // the publisher has finished its WHIP handshake. We retry on
    // `NotFound` for at most `wait_stream_timeout_ms`. Other errors
    // (Internal / Unavailable / Conflict / InvalidArgument) bubble
    // up immediately because they indicate a real problem.
    let retry_interval = std::time::Duration::from_millis(100);
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(wait_stream_timeout_ms);
    let mut subscriber = loop {
        match ctx
            .subscriber_api
            .subscribe(stream_key.clone(), opts.clone())
            .await
        {
            Ok(sub) => break sub,
            Err(SdkError::NotFound(_)) if std::time::Instant::now() < deadline => {
                if cancel.is_cancelled() {
                    return Err(SdkError::Unavailable(
                        "play subscriber cancelled before stream became available".into(),
                    ));
                }
                sleep_for_duration(&ctx.runtime_api, retry_interval).await;
                continue;
            }
            Err(err @ SdkError::NotFound(_)) => {
                // Deadline expired — record the timeout in the play
                // bootstrap stats so the session GET endpoint can
                // surface it as a diagnostic.
                bridges
                    .lock()
                    .record_play_timeout(session_id, wait_stream_timeout_ms);
                warn!(
                    session_id = %session_id,
                    stream = %stream_key,
                    timeout_ms = wait_stream_timeout_ms,
                    "play subscriber wait timeout expired without stream becoming available; \
                     no keyframe could be delivered to the new subscriber"
                );
                return Err(err);
            }
            Err(err) => return Err(err),
        }
    };
    let runtime_api = ctx.runtime_api.clone();
    let timing_runtime = ctx.runtime_api.clone();
    let (mut timing_state, effective_delay_ms) = PlaybackTimingState::new(timing_policy);
    bridges
        .lock()
        .record_play_timing_policy(session_id, timing_policy, effective_delay_ms);
    runtime_api.spawn(Box::pin(async move {
        let mut bootstrap_view = crate::bootstrap::PlayBootstrapView::new();
        let mut skipped_audio_codecs: Vec<cheetah_codec::CodecId> = Vec::new();
        loop {
            let frame = {
                let cancelled = cancel.cancelled().fuse();
                let recv = subscriber.recv().fuse();
                futures::pin_mut!(cancelled, recv);
                futures::select_biased! {
                    _ = cancelled => break,
                    frame = recv => frame,
                }
            };
            match frame {
                    Ok(Some(frame)) => {
                        let codec_mapping =
                            match playback_codec_for_frame(frame.codec, frame.media_kind, audio_policy) {
                                Ok(Some(mapping)) => mapping,
                                Ok(None) => continue,
                                Err(err) => {
                                    if should_skip_unavailable_audio_frame(
                                        frame.codec,
                                        frame.media_kind,
                                        audio_policy,
                                    ) {
                                        if !skipped_audio_codecs.contains(&frame.codec) {
                                            warn!(
                                                session_id = %session_id,
                                                stream = %stream_key,
                                                codec = ?frame.codec,
                                                error = %err,
                                                "dropping unsupported audio frames while keeping WebRTC video playback active"
                                            );
                                            skipped_audio_codecs.push(frame.codec);
                                        }
                                        continue;
                                    }
                                    warn!(
                                        session_id = %session_id,
                                        stream = %stream_key,
                                        error = %err,
                                        "play subscriber stopped because audio output cannot be produced"
                                    );
                                    driver
                                        .send_command(
                                            cheetah_webrtc_driver_tokio::WebRtcDriverCommand::StopSession {
                                                session_id,
                                                reason: cheetah_webrtc_core::WebRtcCloseReason::Internal(
                                                    err.to_string(),
                                                ),
                                            },
                                        )
                                        .await;
                                    break;
                                }
                        };
                        let kind = match frame.media_kind {
                            MediaKind::Audio => cheetah_webrtc_core::WebRtcMediaKind::Audio,
                            MediaKind::Video => cheetah_webrtc_core::WebRtcMediaKind::Video,
                            _ => continue,
                        };
                        let mid = {
                            let guard = bridges.lock();
                            guard.play_track_for(session_id, kind)
                        };
                        let mid = match mid {
                            Some(m) => m,
                            None => {
                                // Track not negotiated yet; drop the
                                // frame silently. A keyframe will be
                                // requested via PLI/FIR once the
                                // remote starts pulling.
                                continue;
                            }
                        };

                        // Step 1: B-frame filter (admission decision).
                        // This is independent of parameter set prepend.
                        if crate::bootstrap::should_filter_bframe(&frame, h264_bframe_filter) {
                            continue;
                        }

                        // Step 2: Codec bootstrap — discover parameter
                        // sets and prepend to keyframes. Delegates all
                        // caching to cheetah-codec's ParameterSetCache.
                        let bootstrap_action = bootstrap_view.process_frame(&frame);
                        let payload = match bootstrap_action {
                            crate::bootstrap::BootstrapAction::Prepended(new_payload) => {
                                new_payload
                            }
                            crate::bootstrap::BootstrapAction::PassThrough
                            | crate::bootstrap::BootstrapAction::KeyframeMissingParameterSets => {
                                frame.payload.clone()
                            }
                        };

                        // Convert canonical microsecond pts/dts into
                        // RTP ticks at the codec clock rate using the
                        // centralized timestamp strategy from cheetah-codec.
                        let clock_rate = codec_mapping.clock_rate;
                        let rtp_ticks = cheetah_codec::compute_rtp_timestamp(
                            &cheetah_codec::RtpTimestampInput {
                                pts: frame.pts,
                                dts: frame.dts,
                                timebase: frame.timebase,
                                media_kind: frame.media_kind,
                                codec: frame.codec,
                                clock_rate,
                                mode: cheetah_codec::RtpTimestampMode::Live,
                                source_frame_number: None,
                                source_pts: None,
                                source_timebase: None,
                                samples_per_frame: cheetah_codec::codec_default_samples_per_frame(frame.codec),
                            },
                        );
                        let delayed_micros =
                            timing_state.apply(frame.pts_us, &timing_runtime).await;
                        let now_micros = std::time::Instant::now()
                            .saturating_duration_since(start_instant)
                            .as_micros() as u64;
                        let send_frame = cheetah_webrtc_core::WebRtcSendFrame {
                            session_id,
                            mid,
                            codec: codec_mapping.codec,
                            clock_rate,
                            rtp_timestamp_ticks: rtp_ticks,
                            rtp_timestamp_denom: clock_rate,
                            random_access: frame
                                .flags
                                .contains(cheetah_codec::FrameFlags::KEY),
                            payload,
                            network_time_micros: now_micros,
                        };
                        // Record GOP bootstrap timing so operators
                        // can observe how quickly first packet,
                        // keyframe and decodable frame land — the
                        // ZLM-equivalent of `WebRtcPlayer::sendConfigFrames`
                        // using `cheetah-codec`'s ParameterSetCache.
                        let has_codec_config = frame
                            .flags
                            .contains(cheetah_codec::FrameFlags::CONFIG);
                        let random_access = send_frame.random_access;
                        bridges.lock().record_play_frame(
                            session_id,
                            now_micros,
                            random_access,
                            has_codec_config,
                        );
                        bridges
                            .lock()
                            .record_play_timing_delay(session_id, delayed_micros);
                        driver
                            .send_command(
                                cheetah_webrtc_driver_tokio::WebRtcDriverCommand::SendFrame(
                                    Box::new(send_frame),
                                ),
                            )
                            .await;
                    }
                    Ok(None) => break,
                    Err(_) => break,
            }
        }
        // Emit diagnostic if the subscriber never received a decodable
        // keyframe. This surfaces the ABL-equivalent "no keyframe"
        // condition as an observable event for operators.
        if !bootstrap_view.has_sent_decodable_keyframe() {
            warn!(
                session_id = %session_id,
                stream = %stream_key,
                "play subscriber closed without receiving a decodable keyframe; \
                 client may have experienced black screen"
            );
        }
        let _ = subscriber.close().await;
    }));
    Ok(())
}

fn should_skip_unavailable_audio_frame(
    codec: cheetah_codec::CodecId,
    media_kind: cheetah_codec::MediaKind,
    audio_policy: PlaybackAudioPolicy,
) -> bool {
    media_kind == cheetah_codec::MediaKind::Audio
        && matches!(
            audio_policy.strategy,
            crate::codec_policy::AudioOutputStrategy::Auto
        )
        && !matches!(
            codec,
            cheetah_codec::CodecId::Opus
                | cheetah_codec::CodecId::G711A
                | cheetah_codec::CodecId::G711U
        )
}

fn map_codec_id_to_kind(codec: cheetah_codec::CodecId) -> Option<WebRtcCodecKind> {
    use cheetah_codec::CodecId;
    Some(match codec {
        CodecId::H264 => WebRtcCodecKind::H264,
        CodecId::H265 => WebRtcCodecKind::H265,
        CodecId::VP8 => WebRtcCodecKind::Vp8,
        CodecId::VP9 => WebRtcCodecKind::Vp9,
        CodecId::AV1 => WebRtcCodecKind::Av1,
        CodecId::Opus => WebRtcCodecKind::Opus,
        CodecId::G711A => WebRtcCodecKind::Pcma,
        CodecId::G711U => WebRtcCodecKind::Pcmu,
        _ => return None,
    })
}

fn playback_codec_for_frame(
    codec: cheetah_codec::CodecId,
    media_kind: cheetah_codec::MediaKind,
    audio_policy: PlaybackAudioPolicy,
) -> Result<Option<PlaybackCodecMapping>, SdkError> {
    if media_kind != cheetah_codec::MediaKind::Audio {
        let Some(mapped) = map_codec_id_to_kind(codec) else {
            return Ok(None);
        };
        let Some(clock_rate) = codec_clock_rate(codec) else {
            return Ok(None);
        };
        return Ok(Some(PlaybackCodecMapping {
            codec: mapped,
            clock_rate,
        }));
    }

    let decision = crate::codec_policy::resolve_audio_output(
        codec,
        audio_policy.profile,
        audio_policy.strategy,
        false,
        true,
    );
    match decision {
        crate::codec_policy::AudioOutputDecision::Passthrough {
            codec, clock_rate, ..
        } => Ok(
            map_codec_id_to_kind(codec).map(|mapped| PlaybackCodecMapping {
                codec: mapped,
                clock_rate,
            }),
        ),
        crate::codec_policy::AudioOutputDecision::TranscodeToOpus { clock_rate, .. } => {
            if codec == cheetah_codec::CodecId::Opus {
                Ok(Some(PlaybackCodecMapping {
                    codec: WebRtcCodecKind::Opus,
                    clock_rate,
                }))
            } else {
                Err(SdkError::Unavailable(format!(
                    "audio codec {codec:?} requires transcoding to Opus, but no Opus transcoder is available in the WebRTC module"
                )))
            }
        }
        crate::codec_policy::AudioOutputDecision::Unavailable { reason, .. } => {
            Err(SdkError::Unavailable(reason.to_string()))
        }
    }
}

fn codec_clock_rate(codec: cheetah_codec::CodecId) -> Option<u32> {
    use cheetah_codec::CodecId;
    Some(match codec {
        CodecId::H264 | CodecId::H265 | CodecId::H266 => 90_000,
        CodecId::VP8 | CodecId::VP9 | CodecId::AV1 => 90_000,
        CodecId::Opus => 48_000,
        CodecId::G711A | CodecId::G711U => 8_000,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SimulcastPolicy;

    fn mid(s: &str) -> MidLabel {
        MidLabel::new(s)
    }

    /// Default thresholds used in tests. Values mirror the module
    /// config defaults (600 / 1800 kbps).
    const DEFAULT_BWE: (u64, u64) = (600_000, 1_800_000);

    fn publish_frame(rtp_timestamp_ticks: u32) -> WebRtcMediaEvent {
        WebRtcMediaEvent::Frame {
            mid: mid("video"),
            rid: None,
            codec: WebRtcCodecKind::Vp8,
            clock_rate: 90_000,
            random_access: true,
            rtp_timestamp_ticks,
            rtp_timestamp_denom: 90_000,
            payload: bytes::Bytes::from_static(&[0x90, 0x00, 0x00, 0x01]),
            network_time_micros: 0,
            meta: cheetah_webrtc_core::WebRtcFrameMeta {
                contiguous: true,
                ..Default::default()
            },
        }
    }

    fn capture_bridge(
        rtcp_based_timestamp: bool,
    ) -> (WebRtcPublishBridge, Arc<Mutex<Vec<Arc<AVFrame>>>>) {
        struct CaptureSink {
            frames: Arc<Mutex<Vec<Arc<AVFrame>>>>,
        }
        impl PublisherSink for CaptureSink {
            fn update_tracks(&self, _: Vec<TrackInfo>) -> Result<(), SdkError> {
                Ok(())
            }
            fn push_frame(
                &self,
                frame: Arc<AVFrame>,
            ) -> Result<cheetah_sdk::DispatchResult, SdkError> {
                self.frames.lock().push(frame);
                Ok(cheetah_sdk::DispatchResult::Accepted)
            }
            fn close(&self) -> Result<(), SdkError> {
                Ok(())
            }
            fn take_keyframe_requests(&self) -> u64 {
                0
            }
        }

        let frames = Arc::new(Mutex::new(Vec::new()));
        let bridge = WebRtcPublishBridge {
            stream_key: StreamKey::new("live", "clock"),
            lease: cheetah_sdk::PublishLease {
                stream_id: cheetah_sdk::StreamId(0),
                stream_key: StreamKey::new("live", "clock"),
                lease_id: 0,
            },
            sink: Box::new(CaptureSink {
                frames: frames.clone(),
            }),
            track_meta: HashMap::new(),
            track_timestamp_epoch: HashMap::new(),
            next_track_id: 1,
            simulcast: SimulcastSelection::new(SimulcastPolicy::Highest, DEFAULT_BWE),
            rtcp_based_timestamp,
            layer_upgrade_pending: false,
            multistream_sinks: HashMap::new(),
            multistream_inflight: std::collections::HashSet::new(),
            publisher_api: None,
        };
        (bridge, frames)
    }

    #[test]
    fn fast_start_timestamp_mode_rebases_first_rtp_timestamp_to_zero() {
        let (mut bridge, frames) = capture_bridge(false);
        bridge.push_frame(publish_frame(900_000));
        bridge.push_frame(publish_frame(903_000));

        let frames = frames.lock();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].pts, 0);
        assert_eq!(frames[0].dts, 0);
        assert_eq!(frames[0].pts_us, 0);
        assert_eq!(frames[1].pts, 3_000);
        assert_eq!(frames[1].dts, 3_000);
        assert_eq!(frames[1].pts_us, 33_333);
    }

    #[test]
    fn rtcp_based_timestamp_mode_preserves_rtp_epoch() {
        let (mut bridge, frames) = capture_bridge(true);
        bridge.push_frame(publish_frame(900_000));

        let frames = frames.lock();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].pts, 900_000);
        assert_eq!(frames[0].dts, 900_000);
        assert_eq!(frames[0].pts_us, 10_000_000);
    }

    #[test]
    fn multistream_admits_all_rids() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::MultiStream, DEFAULT_BWE);
        let mid = mid("video");
        // In MultiStream mode, all RIDs are admitted.
        assert!(sel.admit(&mid, Some("f")));
        assert!(sel.admit(&mid, Some("h")));
        assert!(sel.admit(&mid, Some("q")));
        // Even after seeing all layers, each is still admitted.
        assert!(sel.admit(&mid, Some("f")));
        assert!(sel.admit(&mid, Some("q")));
    }

    #[test]
    fn derive_multistream_key_appends_rid_suffix() {
        let base = StreamKey::new("live", "cam");
        let derived = super::derive_multistream_key(&base, "h");
        assert_eq!(derived.namespace, "live");
        assert_eq!(derived.path, "cam@rid:h");
        assert_eq!(derived.to_string(), "live/cam@rid:h");
    }

    #[test]
    fn derive_multistream_key_handles_complex_path() {
        let base = StreamKey::new("app", "room/stream");
        let derived = super::derive_multistream_key(&base, "q");
        assert_eq!(derived.path, "room/stream@rid:q");
    }

    #[test]
    fn simulcast_admits_everything_when_no_rid() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Highest, DEFAULT_BWE);
        assert!(sel.admit(&mid("0"), None));
        assert!(sel.admit(&mid("0"), Some("")));
    }

    /// Regression: when the bridge re-registers a track because of a
    /// codec change for one mid, the engine `update_tracks` call must
    /// carry the *full* set of known tracks, not just the changed one
    /// — the engine treats it as a complete replace, so passing a
    /// single track would silently drop sibling audio/video tracks.
    #[test]
    fn build_tracks_snapshot_includes_sibling_tracks() {
        // Bypass `acquire`'s engine dependency by constructing the
        // private fields directly. The helper only reads `track_meta`.
        let mut meta = std::collections::HashMap::new();
        meta.insert(
            mid("audio0"),
            TrackMeta {
                track_id: TrackId(1),
                codec: CodecId::Opus,
                media_kind: MediaKind::Audio,
                clock_rate: 48_000,
            },
        );
        meta.insert(
            mid("video0"),
            TrackMeta {
                track_id: TrackId(2),
                codec: CodecId::H264,
                media_kind: MediaKind::Video,
                clock_rate: 90_000,
            },
        );
        // Build a synthetic bridge using only the helper-reachable
        // fields. The sink/lease are not used by `build_tracks_snapshot`.
        struct NoopSink;
        impl PublisherSink for NoopSink {
            fn update_tracks(&self, _: Vec<TrackInfo>) -> Result<(), SdkError> {
                Ok(())
            }
            fn push_frame(&self, _: Arc<AVFrame>) -> Result<cheetah_sdk::DispatchResult, SdkError> {
                Ok(cheetah_sdk::DispatchResult::Accepted)
            }
            fn close(&self) -> Result<(), SdkError> {
                Ok(())
            }
            fn take_keyframe_requests(&self) -> u64 {
                0
            }
        }
        let bridge = WebRtcPublishBridge {
            stream_key: StreamKey::new("live", "demo"),
            lease: cheetah_sdk::PublishLease {
                stream_id: cheetah_sdk::StreamId(0),
                stream_key: StreamKey::new("live", "demo"),
                lease_id: 0,
            },
            sink: Box::new(NoopSink),
            track_meta: meta,
            track_timestamp_epoch: HashMap::new(),
            next_track_id: 3,
            simulcast: SimulcastSelection::new(SimulcastPolicy::Highest, DEFAULT_BWE),
            rtcp_based_timestamp: false,
            layer_upgrade_pending: false,
            multistream_sinks: HashMap::new(),
            multistream_inflight: std::collections::HashSet::new(),
            publisher_api: None,
        };

        // Re-emit video0 with a different codec (e.g., post-renegotiation).
        let new_video = {
            let mut info = TrackInfo::new(TrackId(2), MediaKind::Video, CodecId::VP9, 90_000);
            info.refresh_readiness();
            info
        };
        let snapshot = bridge.build_tracks_snapshot(&mid("video0"), new_video);
        assert_eq!(
            snapshot.len(),
            2,
            "snapshot must keep the audio0 sibling: {snapshot:?}"
        );
        let kinds: Vec<MediaKind> = snapshot.iter().map(|t| t.media_kind).collect();
        assert!(kinds.contains(&MediaKind::Audio));
        assert!(kinds.contains(&MediaKind::Video));
    }

    #[test]
    fn simulcast_highest_picks_ome_full_layer() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Highest, DEFAULT_BWE);
        // Three layers arrive in arbitrary order. After the first
        // frame from each, only the elected RID should keep getting
        // admitted.
        let mid = mid("video");
        // RIDs commonly named `f`, `h`, `q` (chrome's full/half/quarter).
        // OME/ZLM treat these as quality ranks: q < h < f.
        let _ = sel.admit(&mid, Some("f"));
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("q"));
        assert!(sel.admit(&mid, Some("f")));
        assert!(!sel.admit(&mid, Some("h")));
        assert!(!sel.admit(&mid, Some("q")));
    }

    #[test]
    fn rendition_snapshot_reports_current_and_seen_rids_in_quality_order() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Highest, DEFAULT_BWE);
        let mid = mid("video");
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("q"));
        let _ = sel.admit(&mid, Some("f"));

        let snapshot = sel.rendition_snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].mid, "video");
        assert_eq!(snapshot[0].current_rid.as_deref(), Some("f"));
        assert_eq!(
            snapshot[0].seen_rids,
            vec!["q".to_string(), "h".to_string(), "f".to_string()]
        );
    }

    #[test]
    fn simulcast_lowest_picks_ome_quarter_layer() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Lowest, DEFAULT_BWE);
        let mid = mid("video");
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("f"));
        let _ = sel.admit(&mid, Some("q"));
        assert!(sel.admit(&mid, Some("q")));
        assert!(!sel.admit(&mid, Some("f")));
        assert!(!sel.admit(&mid, Some("h")));
    }

    #[test]
    fn simulcast_rid_pinning_admits_only_named_layer() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Rid("h".into()), DEFAULT_BWE);
        let mid = mid("video");
        // Before `h` is seen, nothing is admitted.
        assert!(!sel.admit(&mid, Some("f")));
        // Once `h` is seen, only `h` is admitted.
        let _ = sel.admit(&mid, Some("h"));
        assert!(sel.admit(&mid, Some("h")));
        assert!(!sel.admit(&mid, Some("f")));
    }

    /// Adaptive policy without a BWE estimate falls back to "highest"
    /// behaviour so a session does not stall before BWE arrives.
    #[test]
    fn simulcast_adaptive_without_estimate_acts_like_highest() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        let mid = mid("video");
        let _ = sel.admit(&mid, Some("f"));
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("q"));
        assert!(
            sel.admit(&mid, Some("f")),
            "adaptive without BWE elects the highest OME RID"
        );
        assert!(!sel.admit(&mid, Some("q")));
    }

    /// Adaptive policy with a low estimate elects the lowest layer.
    #[test]
    fn simulcast_adaptive_low_bwe_picks_lowest_layer() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        let mid = mid("video");
        let _ = sel.admit(&mid, Some("f"));
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("q"));
        // 200 kbps is well below the 600 kbps low threshold.
        sel.set_bwe_estimate(200_000);
        assert!(sel.admit(&mid, Some("q")));
        assert!(!sel.admit(&mid, Some("f")));
    }

    /// Adaptive policy with a high estimate elects the highest layer.
    #[test]
    fn simulcast_adaptive_high_bwe_picks_highest_layer() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        let mid = mid("video");
        let _ = sel.admit(&mid, Some("f"));
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("q"));
        // 5 Mbps exceeds the 1.8 Mbps high threshold.
        sel.set_bwe_estimate(5_000_000);
        assert!(sel.admit(&mid, Some("f")));
        assert!(!sel.admit(&mid, Some("q")));
    }

    /// Adaptive policy in the mid range elects the middle layer when
    /// three layers are available.
    #[test]
    fn simulcast_adaptive_mid_bwe_picks_middle_layer() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        let mid = mid("video");
        let _ = sel.admit(&mid, Some("f"));
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("q"));
        // 1 Mbps is between the 600 kbps low and 1.8 Mbps high.
        sel.set_bwe_estimate(1_000_000);
        // OME quality order: ["q", "h", "f"]; middle index = 1 => "h".
        assert!(sel.admit(&mid, Some("h")));
        assert!(!sel.admit(&mid, Some("f")));
        assert!(!sel.admit(&mid, Some("q")));
    }

    /// REMB cap should pull the elected layer down when it is
    /// tighter than the local BWE estimate. Without this, the
    /// adaptive policy would silently overshoot the receiver's
    /// suggested ceiling.
    #[test]
    fn simulcast_adaptive_remb_cap_overrides_higher_bwe_estimate() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        let mid = mid("video");
        let _ = sel.admit(&mid, Some("f"));
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("q"));
        // Local BWE says we have plenty of headroom.
        sel.set_bwe_estimate(5_000_000);
        // But the remote receiver requests a much tighter cap.
        sel.set_remb_cap(300_000);
        // Effective cap is min(5_000_000, 300_000) = 300_000, which
        // is below the 600 kbps low threshold ⇒ pick the lowest.
        assert!(sel.admit(&mid, Some("q")));
        assert!(!sel.admit(&mid, Some("f")));
        assert!(!sel.admit(&mid, Some("h")));
    }

    /// When the local BWE estimate is the tighter constraint, REMB
    /// should not relax it. The effective cap is `min(bwe, remb)`.
    #[test]
    fn simulcast_adaptive_remb_cap_does_not_relax_low_bwe() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        let mid = mid("video");
        let _ = sel.admit(&mid, Some("f"));
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("q"));
        // Local BWE indicates congestion.
        sel.set_bwe_estimate(200_000);
        // REMB suggests a higher cap (e.g., the receiver has not yet
        // observed the local-side congestion).
        sel.set_remb_cap(3_000_000);
        // min(200_000, 3_000_000) = 200_000 ⇒ still elect lowest.
        assert!(sel.admit(&mid, Some("q")));
        assert!(!sel.admit(&mid, Some("f")));
    }

    /// Sanity check: REMB without BWE acts as the sole cap.
    #[test]
    fn simulcast_adaptive_remb_only_drives_election() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        let mid = mid("video");
        let _ = sel.admit(&mid, Some("f"));
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("q"));
        sel.set_remb_cap(200_000);
        assert!(sel.admit(&mid, Some("q")));
        assert!(!sel.admit(&mid, Some("f")));
    }

    /// NACK storm detector trips on a single delta exceeding the
    /// threshold and pins the simulcast election to the lowest
    /// layer regardless of the BWE / REMB cap.
    #[test]
    fn nack_storm_pins_to_lowest_layer_until_recovery() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        let mid = mid("video");
        let _ = sel.admit(&mid, Some("f"));
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("q"));
        // Plenty of headroom from BWE/REMB → adaptive policy would
        // normally elect "f" (highest).
        sel.set_bwe_estimate(5_000_000);
        sel.set_remb_cap(5_000_000);
        assert!(sel.admit(&mid, Some("f")));

        // Trip the storm: 60 NACKs in one sample exceeds the
        // default threshold of 50.
        let storm = sel.observe_nack_in(60);
        assert!(storm, "first sample with delta=60 must trip the storm");

        // Election is forced to the lowest layer.
        assert!(sel.admit(&mid, Some("q")));
        assert!(!sel.admit(&mid, Some("f")));
        assert!(!sel.admit(&mid, Some("h")));
    }

    /// The recovery window decays one sample at a time. After
    /// `nack_storm_recovery_samples` quiet samples, the adaptive
    /// policy returns to BWE/REMB-driven selection.
    #[test]
    fn nack_storm_recovery_window_lifts_force_lowest_after_decay() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        let mid = mid("video");
        let _ = sel.admit(&mid, Some("f"));
        let _ = sel.admit(&mid, Some("h"));
        let _ = sel.admit(&mid, Some("q"));
        sel.set_bwe_estimate(5_000_000);

        assert!(sel.observe_nack_in(60));
        assert!(sel.in_nack_storm());

        // Five quiet samples (matches DEFAULT_NACK_STORM_RECOVERY_SAMPLES).
        // observe_nack_in with a small delta decays the recovery
        // counter by one each call.
        for sample in 1..=5 {
            // 60 + sample*1 = 61, 62, ... — delta 1 each, well below the
            // 50/sample threshold so no new storm is triggered.
            let value = 60 + sample;
            assert!(
                !sel.observe_nack_in(value as u64),
                "small delta must not retrigger the storm at sample {sample}"
            );
        }
        // After 5 quiet samples we should be out of recovery.
        assert!(!sel.in_nack_storm());
        // And election follows BWE again — high BWE → highest layer.
        assert!(sel.admit(&mid, Some("f")));
    }

    /// Sub-threshold sustained loss does not flip the storm flag.
    #[test]
    fn nack_storm_sub_threshold_delta_does_not_trip() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        // Default threshold is 50 NACKs/sample. 10 per sample is
        // background loss; we should not trip.
        for i in 1..=10 {
            assert!(
                !sel.observe_nack_in(i * 10),
                "sample {i} (delta=10) must not trip the storm"
            );
        }
        assert!(!sel.in_nack_storm());
    }

    /// Repeated bursts: a second storm during recovery should reset
    /// the recovery window, keeping the policy pinned to lowest.
    #[test]
    fn nack_storm_repeated_burst_resets_recovery_window() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        // First burst trips the storm.
        assert!(sel.observe_nack_in(60));
        assert!(sel.in_nack_storm());
        // Two quiet samples decay the recovery window.
        assert!(!sel.observe_nack_in(61));
        assert!(!sel.observe_nack_in(62));
        // Still in recovery.
        assert!(sel.in_nack_storm());
        // Second burst arrives — should re-trip and reset to full
        // recovery window.
        assert!(sel.observe_nack_in(120));
        assert!(sel.in_nack_storm());
        // Now we need the full DEFAULT_NACK_STORM_RECOVERY_SAMPLES (5)
        // quiet samples to exit recovery.
        for i in 0..5 {
            sel.observe_nack_in(120 + (i as u64));
        }
        assert!(!sel.in_nack_storm());
    }

    /// NACK count wraparound: in the unlikely case that the cumulative
    /// NACK count wraps (e.g., counter overflow or session restart),
    /// the saturating subtraction must not trigger a false storm.
    #[test]
    fn nack_storm_does_not_trip_on_count_decrease() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        // Establish a baseline at 1000 (this trips the storm because
        // the initial delta is large).
        sel.observe_nack_in(1000);
        // Wait out the recovery window so we're back to normal.
        for i in 0..6 {
            sel.observe_nack_in(1000 + i);
        }
        assert!(!sel.in_nack_storm());
        // Counter "decreases" (e.g., session restart or counter reset).
        // saturating_sub returns 0, so no storm should trip.
        assert!(!sel.observe_nack_in(500));
        assert!(!sel.in_nack_storm());
    }

    /// Boundary: a delta exactly at the threshold should trip.
    #[test]
    fn nack_storm_trips_at_exact_threshold() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        // Default threshold is 50; delta=50 should trip.
        assert!(sel.observe_nack_in(50));
        assert!(sel.in_nack_storm());
    }

    /// Boundary: a delta one below the threshold should not trip.
    #[test]
    fn nack_storm_does_not_trip_below_threshold() {
        let mut sel = SimulcastSelection::new(SimulcastPolicy::Adaptive, DEFAULT_BWE);
        // Delta=49 should not trip.
        assert!(!sel.observe_nack_in(49));
        assert!(!sel.in_nack_storm());
    }
}

#[cfg(test)]
mod bootstrap_stats_tests {
    use super::*;

    fn sid(value: u64) -> WebRtcSessionId {
        WebRtcSessionId::new(value)
    }

    #[test]
    fn first_frame_records_first_frame_micros() {
        let mut reg = WebRtcBridgeRegistry::default();
        reg.record_play_frame(sid(1), 1_500, false, false);
        let stats = reg.play_stats(sid(1)).expect("stats present");
        assert_eq!(stats.first_frame_micros, Some(1_500));
        assert_eq!(stats.first_keyframe_micros, None);
        assert_eq!(stats.first_decodable_micros, None);
        assert_eq!(stats.frames_forwarded, 1);
        assert_eq!(stats.keyframes_forwarded, 0);
    }

    #[test]
    fn first_keyframe_marks_decodable_too() {
        let mut reg = WebRtcBridgeRegistry::default();
        reg.record_play_frame(sid(1), 1_000, false, false);
        reg.record_play_frame(sid(1), 2_500, true, false);
        let stats = reg.play_stats(sid(1)).expect("stats present");
        assert_eq!(stats.first_frame_micros, Some(1_000));
        assert_eq!(stats.first_keyframe_micros, Some(2_500));
        // A keyframe doubles as a decodable bootstrap point.
        assert_eq!(stats.first_decodable_micros, Some(2_500));
        assert_eq!(stats.frames_forwarded, 2);
        assert_eq!(stats.keyframes_forwarded, 1);
    }

    #[test]
    fn config_frame_alone_marks_decodable_without_keyframe() {
        // A FrameFlags::CONFIG carrier (e.g. SPS/PPS) is enough to
        // mark the stream as decodable from bootstrap perspective,
        // even before the first keyframe lands. ZLM uses the same
        // bootstrap rule when sending config frames ahead of the
        // first IDR.
        let mut reg = WebRtcBridgeRegistry::default();
        reg.record_play_frame(sid(1), 800, false, true);
        let stats = reg.play_stats(sid(1)).expect("stats present");
        assert_eq!(stats.first_decodable_micros, Some(800));
        assert_eq!(stats.first_keyframe_micros, None);
    }

    #[test]
    fn first_frame_micros_is_stable_across_multiple_frames() {
        let mut reg = WebRtcBridgeRegistry::default();
        reg.record_play_frame(sid(1), 100, false, false);
        reg.record_play_frame(sid(1), 200, false, false);
        reg.record_play_frame(sid(1), 300, true, false);
        let stats = reg.play_stats(sid(1)).expect("stats present");
        // first_frame_micros must not regress to a later frame.
        assert_eq!(stats.first_frame_micros, Some(100));
        assert_eq!(stats.first_keyframe_micros, Some(300));
        assert_eq!(stats.frames_forwarded, 3);
    }

    #[test]
    fn remove_play_clears_bootstrap_stats() {
        let mut reg = WebRtcBridgeRegistry::default();
        reg.record_play_frame(sid(1), 100, true, true);
        reg.insert_play(sid(1), cheetah_sdk::CancellationToken::new());
        assert!(reg.play_stats(sid(1)).is_some());
        reg.remove_play(sid(1));
        assert!(reg.play_stats(sid(1)).is_none());
    }

    #[test]
    fn play_timing_policy_and_delay_are_observable() {
        let mut reg = WebRtcBridgeRegistry::default();
        reg.record_play_timing_policy(
            sid(2),
            PlaybackTimingPolicy {
                jitter_buffer_ms: 120,
                playout_delay_min_ms: 80,
                playout_delay_max_ms: 200,
            },
            120,
        );
        reg.record_play_timing_delay(sid(2), 33_000);
        reg.record_play_timing_delay(sid(2), 0);
        let stats = reg.play_stats(sid(2)).expect("stats present");
        assert_eq!(stats.jitter_buffer_ms, 120);
        assert_eq!(stats.playout_delay_min_ms, 80);
        assert_eq!(stats.playout_delay_max_ms, 200);
        assert_eq!(stats.effective_playout_delay_ms, 120);
        assert_eq!(stats.delayed_frames, 1);
        assert_eq!(stats.delayed_total_micros, 33_000);
    }

    #[test]
    fn playback_audio_policy_rejects_aac_when_transcoding_is_unavailable() {
        let policy = PlaybackAudioPolicy {
            profile: crate::config::CodecProfileWire::Browser,
            strategy: crate::codec_policy::AudioOutputStrategy::TranscodeToOpus,
        };

        let err = playback_codec_for_frame(cheetah_codec::CodecId::AAC, MediaKind::Audio, policy)
            .expect_err("AAC should not be silently dropped when transcode is configured");
        assert!(err.to_string().contains("requires transcoding to Opus"));
    }

    #[test]
    fn default_audio_policy_skips_aac_without_closing_video_playback() {
        let policy = PlaybackAudioPolicy {
            profile: crate::config::CodecProfileWire::Browser,
            strategy: crate::codec_policy::AudioOutputStrategy::Auto,
        };

        let err = playback_codec_for_frame(cheetah_codec::CodecId::AAC, MediaKind::Audio, policy)
            .expect_err("AAC still requires unavailable Opus transcoding");
        assert!(err.to_string().contains("requires transcoding to Opus"));
        assert!(should_skip_unavailable_audio_frame(
            cheetah_codec::CodecId::AAC,
            MediaKind::Audio,
            policy
        ));
        assert!(!should_skip_unavailable_audio_frame(
            cheetah_codec::CodecId::AAC,
            MediaKind::Audio,
            PlaybackAudioPolicy {
                profile: crate::config::CodecProfileWire::Browser,
                strategy: crate::codec_policy::AudioOutputStrategy::TranscodeToOpus,
            }
        ));
    }

    #[test]
    fn playback_audio_policy_passes_g711_through_when_configured() {
        let policy = PlaybackAudioPolicy {
            profile: crate::config::CodecProfileWire::Browser,
            strategy: crate::codec_policy::AudioOutputStrategy::Passthrough,
        };

        let mapped =
            playback_codec_for_frame(cheetah_codec::CodecId::G711A, MediaKind::Audio, policy)
                .expect("G711A passthrough should be accepted")
                .expect("G711A should map to a WebRTC codec");
        assert_eq!(mapped.codec, WebRtcCodecKind::Pcma);
        assert_eq!(mapped.clock_rate, 8_000);
    }
}
