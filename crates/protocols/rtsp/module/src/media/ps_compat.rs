use cheetah_codec::{CodecId, PsPacket, PsStreamKind, TrackInfo};
use cheetah_rtsp_driver_tokio::RtspConnectionId;
use tracing::warn;

use crate::session::PublishSession;

const MP2P_PS_PROBE_MAX_PAYLOAD_BYTES: usize = 4 * 1024;
const MP2P_PS_PROBE_MAX_PES_SCAN: usize = 16;
const MP2P_PS_PROBE_ALERT_THRESHOLD: u64 = 256;

/// `Mp2pPsProbeOutcome` enumeration.
/// `Mp2pPsProbeOutcome` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mp2pPsProbeOutcome {
    /// `NoPsPayload` variant.
    /// `NoPsPayload` 变体.
    NoPsPayload,
    /// `PrivateOnly` variant.
    /// `PrivateOnly` 变体.
    PrivateOnly,
    /// `ElementaryStreamDetected` variant.
    /// `ElementaryStreamDetected` 变体.
    ElementaryStreamDetected,
}

/// Returns `true` if `mp2p_probe_track` is true.
/// 返回 `真` 如果 `mp2p_probe_track` is 真.
pub(crate) fn is_mp2p_probe_track(track: &TrackInfo) -> bool {
    track.codec == CodecId::Unknown && crate::sdp::is_mp2p_probe_track(track)
}

/// `probe_mp2p_ps_payload` function.
/// `probe_mp2p_ps_payload` 函数.
pub(crate) fn probe_mp2p_ps_payload(payload: &[u8]) -> Mp2pPsProbeOutcome {
    if payload.is_empty() {
        return Mp2pPsProbeOutcome::NoPsPayload;
    }
    let parsed = PsPacket::parse_bounded(
        payload,
        MP2P_PS_PROBE_MAX_PAYLOAD_BYTES,
        MP2P_PS_PROBE_MAX_PES_SCAN,
    );
    if parsed.pes.is_empty() {
        return Mp2pPsProbeOutcome::NoPsPayload;
    }

    let mut saw_private = false;
    for pes in &parsed.pes {
        match pes.kind {
            PsStreamKind::Video | PsStreamKind::Audio => {
                if !pes.payload.is_empty() {
                    return Mp2pPsProbeOutcome::ElementaryStreamDetected;
                }
            }
            PsStreamKind::Private => {
                saw_private = true;
            }
        }
    }
    if saw_private {
        Mp2pPsProbeOutcome::PrivateOnly
    } else {
        Mp2pPsProbeOutcome::NoPsPayload
    }
}

/// `record_mp2p_probe_drop` function.
/// `record_mp2p_probe_drop` 函数.
pub(crate) fn record_mp2p_probe_drop(
    publish: &mut PublishSession,
    connection_id: RtspConnectionId,
    track: &TrackInfo,
    probe_outcome: Mp2pPsProbeOutcome,
) {
    let drop_count = publish
        .compat_probe_drop_counts
        .entry(track.track_id)
        .and_modify(|count| *count = count.saturating_add(1))
        .or_insert(1);

    let should_sample = cheetah_codec::should_sample_timestamp_repair(*drop_count);
    let should_threshold =
        cheetah_codec::should_emit_alert_threshold(*drop_count, MP2P_PS_PROBE_ALERT_THRESHOLD);
    if should_sample || should_threshold {
        warn!(
            connection_id,
            stream_key = %publish.lease.stream_key,
            track_id = track.track_id.0,
            payload_type = ?track.payload_type,
            drop_count = *drop_count,
            probe_outcome = ?probe_outcome,
            "rtsp publish mp2p/ps compat probe packet dropped (not ingested to engine)"
        );
    }
}
