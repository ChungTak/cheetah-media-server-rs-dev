use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use cheetah_codec::{MuteAudioMaker, ParameterSetCache, TimestampNormalizer, TrackId, TrackInfo};
use cheetah_rtsp_driver_tokio::RtspConnectionId;
use cheetah_sdk::{
    AsyncUdpSocket, CancellationToken, JoinHandle as RuntimeJoinHandle, PublishLease,
    PublisherSink, StreamKey,
};

/// `SessionMode` enumeration.
/// `SessionMode` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    /// `Publish` variant.
    /// `Publish` 变体.
    Publish,
    /// `Play` variant.
    /// `Play` 变体.
    Play,
}

/// `PublishTrackClock` data structure.
/// `PublishTrackClock` 数据结构.
#[derive(Default)]
pub struct PublishTrackClock {
    /// `base_seq` field.
    /// `base_seq` 字段.
    pub base_seq: Option<u16>,
    /// `max_seq` field of type `u16`.
    /// `max_seq` 字段，类型为 `u16`.
    pub max_seq: u16,
    /// `cycles` field of type `u32`.
    /// `cycles` 字段，类型为 `u32`.
    pub cycles: u32,
    /// `received_packets` field of type `u32`.
    /// `received_packets` 字段，类型为 `u32`.
    pub received_packets: u32,
    /// `jitter` field of type `u32`.
    /// `jitter` 字段，类型为 `u32`.
    pub jitter: u32,
    /// `last_transit` field.
    /// `last_transit` 字段.
    pub last_transit: Option<i64>,
    /// `last_sr_lsr` field.
    /// `last_sr_lsr` 字段.
    pub last_sr_lsr: Option<u32>,
    /// `last_sr_unix_micros` field.
    /// `last_sr_unix_micros` 字段.
    pub last_sr_unix_micros: Option<u64>,
    /// `last_rr_expected` field of type `u32`.
    /// `last_rr_expected` 字段，类型为 `u32`.
    pub last_rr_expected: u32,
    /// `last_rr_received` field of type `u32`.
    /// `last_rr_received` 字段，类型为 `u32`.
    pub last_rr_received: u32,
}

/// `PublishH264Depacketizer` data structure.
/// `PublishH264Depacketizer` 数据结构.
#[derive(Default)]
pub struct PublishH264Depacketizer {
    /// `fu_buffer` field.
    /// `fu_buffer` 字段.
    pub fu_buffer: Vec<u8>,
    /// `access_unit` field.
    /// `access_unit` 字段.
    pub access_unit: Vec<u8>,
    /// `access_unit_timestamp` field.
    /// `access_unit_timestamp` 字段.
    pub access_unit_timestamp: Option<u32>,
    /// `access_unit_last_sequence` field.
    /// `access_unit_last_sequence` 字段.
    pub access_unit_last_sequence: Option<u16>,
    /// `access_unit_keyframe` field of type `bool`.
    /// `access_unit_keyframe` 字段，类型为 `bool`.
    pub access_unit_keyframe: bool,
    /// `access_unit_marker_seen` field of type `bool`.
    /// `access_unit_marker_seen` 字段，类型为 `bool`.
    pub access_unit_marker_seen: bool,
}

/// `PublishH265Depacketizer` data structure.
/// `PublishH265Depacketizer` 数据结构.
#[derive(Default)]
pub struct PublishH265Depacketizer {
    /// `fu_buffer` field.
    /// `fu_buffer` 字段.
    pub fu_buffer: Vec<u8>,
    /// `access_unit` field.
    /// `access_unit` 字段.
    pub access_unit: Vec<u8>,
    /// `access_unit_timestamp` field.
    /// `access_unit_timestamp` 字段.
    pub access_unit_timestamp: Option<u32>,
    /// `access_unit_last_sequence` field.
    /// `access_unit_last_sequence` 字段.
    pub access_unit_last_sequence: Option<u16>,
    /// `access_unit_keyframe` field of type `bool`.
    /// `access_unit_keyframe` 字段，类型为 `bool`.
    pub access_unit_keyframe: bool,
    /// `access_unit_marker_seen` field of type `bool`.
    /// `access_unit_marker_seen` 字段，类型为 `bool`.
    pub access_unit_marker_seen: bool,
}

/// `PublishAv1Depacketizer` data structure.
/// `PublishAv1Depacketizer` 数据结构.
#[derive(Default)]
pub struct PublishAv1Depacketizer {
    /// `access_unit` field.
    /// `access_unit` 字段.
    pub access_unit: Vec<u8>,
    /// `current_obu` field.
    /// `current_obu` 字段.
    pub current_obu: Vec<u8>,
    /// `access_unit_timestamp` field.
    /// `access_unit_timestamp` 字段.
    pub access_unit_timestamp: Option<u32>,
    /// `access_unit_last_sequence` field.
    /// `access_unit_last_sequence` 字段.
    pub access_unit_last_sequence: Option<u16>,
    /// `access_unit_keyframe` field of type `bool`.
    /// `access_unit_keyframe` 字段，类型为 `bool`.
    pub access_unit_keyframe: bool,
    /// `access_unit_marker_seen` field of type `bool`.
    /// `access_unit_marker_seen` 字段，类型为 `bool`.
    pub access_unit_marker_seen: bool,
}

/// `PublishVp9Depacketizer` data structure.
/// `PublishVp9Depacketizer` 数据结构.
#[derive(Default)]
pub struct PublishVp9Depacketizer {
    /// `access_unit` field.
    /// `access_unit` 字段.
    pub access_unit: Vec<u8>,
    /// `access_unit_timestamp` field.
    /// `access_unit_timestamp` 字段.
    pub access_unit_timestamp: Option<u32>,
    /// `access_unit_last_sequence` field.
    /// `access_unit_last_sequence` 字段.
    pub access_unit_last_sequence: Option<u16>,
    /// `access_unit_keyframe` field of type `bool`.
    /// `access_unit_keyframe` 字段，类型为 `bool`.
    pub access_unit_keyframe: bool,
}

/// `PublishVp8Depacketizer` data structure.
/// `PublishVp8Depacketizer` 数据结构.
#[derive(Default)]
pub struct PublishVp8Depacketizer {
    /// `access_unit` field.
    /// `access_unit` 字段.
    pub access_unit: Vec<u8>,
    /// `access_unit_timestamp` field.
    /// `access_unit_timestamp` 字段.
    pub access_unit_timestamp: Option<u32>,
    /// `access_unit_last_sequence` field.
    /// `access_unit_last_sequence` 字段.
    pub access_unit_last_sequence: Option<u16>,
    /// `access_unit_keyframe` field of type `bool`.
    /// `access_unit_keyframe` 字段，类型为 `bool`.
    pub access_unit_keyframe: bool,
}

/// `PublishTrackTimestampState` data structure.
/// `PublishTrackTimestampState` 数据结构.
pub struct PublishTrackTimestampState {
    /// `normalizer` field of type `TimestampNormalizer`.
    /// `normalizer` 字段，类型为 `TimestampNormalizer`.
    pub normalizer: TimestampNormalizer,
    /// `repair_count` field of type `u64`.
    /// `repair_count` 字段，类型为 `u64`.
    pub repair_count: u64,
    /// `source_disorder_count` field of type `u64`.
    /// `source_disorder_count` 字段，类型为 `u64`.
    pub source_disorder_count: u64,
    /// `discontinuity_count` field of type `u64`.
    /// `discontinuity_count` 字段，类型为 `u64`.
    pub discontinuity_count: u64,
}

/// `RtcpReceiverMetrics` data structure.
/// `RtcpReceiverMetrics` 数据结构.
pub struct RtcpReceiverMetrics {
    /// `fraction_lost` field of type `u8`.
    /// `fraction_lost` 字段，类型为 `u8`.
    pub fraction_lost: u8,
    /// `cumulative_lost` field of type `u32`.
    /// `cumulative_lost` 字段，类型为 `u32`.
    pub cumulative_lost: u32,
    /// `extended_highest_seq` field of type `u32`.
    /// `extended_highest_seq` 字段，类型为 `u32`.
    pub extended_highest_seq: u32,
    /// `jitter` field of type `u32`.
    /// `jitter` 字段，类型为 `u32`.
    pub jitter: u32,
    /// `lsr` field of type `u32`.
    /// `lsr` 字段，类型为 `u32`.
    pub lsr: u32,
    /// `dlsr` field of type `u32`.
    /// `dlsr` 字段，类型为 `u32`.
    pub dlsr: u32,
}

impl PublishTrackClock {
    /// `on_rtp_packet` function.
    /// `on_rtp_packet` 函数.
    pub fn on_rtp_packet(
        &mut self,
        seq: u16,
        timestamp: u32,
        clock_rate: u32,
        arrival_unix_micros: u64,
    ) {
        self.received_packets = self.received_packets.wrapping_add(1);
        match self.base_seq {
            None => {
                self.base_seq = Some(seq);
                self.max_seq = seq;
            }
            Some(_) => {
                let wrap_forward = seq < self.max_seq && self.max_seq.wrapping_sub(seq) > 0x8000;
                if wrap_forward {
                    self.cycles = self.cycles.wrapping_add(1 << 16);
                    self.max_seq = seq;
                } else if seq > self.max_seq {
                    self.max_seq = seq;
                }
            }
        }

        let arrival_rtp = ((arrival_unix_micros as u128).saturating_mul(u128::from(clock_rate))
            / 1_000_000u128) as i64;
        let transit = arrival_rtp.saturating_sub(i64::from(timestamp));
        if let Some(last_transit) = self.last_transit {
            let delta = transit.saturating_sub(last_transit).unsigned_abs() as i64;
            let jitter = i64::from(self.jitter);
            let updated = jitter.saturating_add((delta.saturating_sub(jitter)) / 16);
            self.jitter = updated.max(0).min(i64::from(u32::MAX)) as u32;
        }
        self.last_transit = Some(transit);
    }

    /// `note_sender_report` function.
    /// `note_sender_report` 函数.
    pub fn note_sender_report(&mut self, lsr: u32, arrival_unix_micros: u64) {
        self.last_sr_lsr = Some(lsr);
        self.last_sr_unix_micros = Some(arrival_unix_micros);
    }

    /// Builds `receiver_metrics` output.
    /// 构建 `receiver_metrics` 输出.
    pub fn build_receiver_metrics(&mut self, now_unix_micros: u64) -> RtcpReceiverMetrics {
        let expected = self.expected_packets();
        let expected_interval = expected.saturating_sub(self.last_rr_expected);
        let received_interval = self.received_packets.saturating_sub(self.last_rr_received);
        let lost_interval =
            i64::from(expected_interval).saturating_sub(i64::from(received_interval));
        let fraction_lost = if expected_interval == 0 || lost_interval <= 0 {
            0
        } else {
            (((lost_interval as u128).saturating_mul(256u128)) / u128::from(expected_interval))
                .min(u128::from(u8::MAX)) as u8
        };
        self.last_rr_expected = expected;
        self.last_rr_received = self.received_packets;

        let cumulative_lost = i64::from(expected).saturating_sub(i64::from(self.received_packets));
        let cumulative_lost = cumulative_lost.clamp(0, 0x7f_ffff) as u32;

        let (lsr, dlsr) = match (self.last_sr_lsr, self.last_sr_unix_micros) {
            (Some(lsr), Some(last_sr_unix_micros)) => {
                let delta_us = now_unix_micros.saturating_sub(last_sr_unix_micros);
                let dlsr = ((u128::from(delta_us).saturating_mul(65_536u128)) / 1_000_000u128)
                    .min(u128::from(u32::MAX)) as u32;
                (lsr, dlsr)
            }
            _ => (0, 0),
        };

        RtcpReceiverMetrics {
            fraction_lost,
            cumulative_lost,
            extended_highest_seq: self.cycles.wrapping_add(u32::from(self.max_seq)),
            jitter: self.jitter,
            lsr,
            dlsr,
        }
    }

    fn expected_packets(&self) -> u32 {
        if let Some(base_seq) = self.base_seq {
            self.cycles
                .wrapping_add(u32::from(self.max_seq))
                .wrapping_sub(u32::from(base_seq))
                .wrapping_add(1)
        } else {
            0
        }
    }
}

/// `PublishSession` data structure.
/// `PublishSession` 数据结构.
pub struct PublishSession {
    /// `cancel` field of type `CancellationToken`.
    /// `cancel` 字段，类型为 `CancellationToken`.
    pub cancel: CancellationToken,
    /// `lease` field of type `PublishLease`.
    /// `lease` 字段，类型为 `PublishLease`.
    pub lease: PublishLease,
    /// `sink` field.
    /// `sink` 字段.
    pub sink: Box<dyn PublisherSink>,
    /// `record_started` field of type `bool`.
    /// `record_started` 字段，类型为 `bool`.
    pub record_started: bool,
    /// `pre_record_rtp_drop_count` field of type `u64`.
    /// `pre_record_rtp_drop_count` 字段，类型为 `u64`.
    pub pre_record_rtp_drop_count: u64,
    /// `timestamp_repair_alert_threshold` field of type `u64`.
    /// `timestamp_repair_alert_threshold` 字段，类型为 `u64`.
    pub timestamp_repair_alert_threshold: u64,
    /// `queue_drop_alert_threshold` field of type `u64`.
    /// `queue_drop_alert_threshold` 字段，类型为 `u64`.
    pub queue_drop_alert_threshold: u64,
    /// `queue_drop_counts` field.
    /// `queue_drop_counts` 字段.
    pub queue_drop_counts: HashMap<TrackId, u64>,
    /// `unsupported_codec_drop_counts` field.
    /// `unsupported_codec_drop_counts` 字段.
    pub unsupported_codec_drop_counts: HashMap<TrackId, u64>,
    /// `compat_probe_drop_counts` field.
    /// `compat_probe_drop_counts` 字段.
    pub compat_probe_drop_counts: HashMap<TrackId, u64>,
    /// `tracks` field.
    /// `tracks` 字段.
    pub tracks: HashMap<TrackId, TrackInfo>,
    /// `track_channels` field.
    /// `track_channels` 字段.
    pub track_channels: HashMap<u8, TrackId>,
    /// `rtcp_channels` field.
    /// `rtcp_channels` 字段.
    pub rtcp_channels: HashMap<u8, TrackId>,
    /// `clocks` field.
    /// `clocks` 字段.
    pub clocks: HashMap<TrackId, PublishTrackClock>,
    /// `h264_depacketizers` field.
    /// `h264_depacketizers` 字段.
    pub h264_depacketizers: HashMap<TrackId, PublishH264Depacketizer>,
    /// `h265_depacketizers` field.
    /// `h265_depacketizers` 字段.
    pub h265_depacketizers: HashMap<TrackId, PublishH265Depacketizer>,
    /// `av1_depacketizers` field.
    /// `av1_depacketizers` 字段.
    pub av1_depacketizers: HashMap<TrackId, PublishAv1Depacketizer>,
    /// `vp9_depacketizers` field.
    /// `vp9_depacketizers` 字段.
    pub vp9_depacketizers: HashMap<TrackId, PublishVp9Depacketizer>,
    /// `vp8_depacketizers` field.
    /// `vp8_depacketizers` 字段.
    pub vp8_depacketizers: HashMap<TrackId, PublishVp8Depacketizer>,
    /// `track_last_frame_timestamps` field.
    /// `track_last_frame_timestamps` 字段.
    pub track_last_frame_timestamps: HashMap<TrackId, i64>,
    /// `timestamp_normalizers` field.
    /// `timestamp_normalizers` 字段.
    pub timestamp_normalizers: HashMap<TrackId, PublishTrackTimestampState>,
    /// `video_parameter_sets` field.
    /// `video_parameter_sets` 字段.
    pub video_parameter_sets: HashMap<TrackId, ParameterSetCache>,
    /// `udp_tracks` field.
    /// `udp_tracks` 字段.
    pub udp_tracks: HashMap<TrackId, PublishUdpTrack>,
    /// `udp_task_handles` field.
    /// `udp_task_handles` 字段.
    pub udp_task_handles: Vec<Box<dyn RuntimeJoinHandle>>,
    /// `mute_audio_maker` field.
    /// `mute_audio_maker` 字段.
    pub mute_audio_maker: Option<MuteAudioMaker>,
    /// `codec_probed` field.
    /// `codec_probed` 字段.
    pub codec_probed: HashSet<TrackId>,
}

/// `PlayTransport` enumeration.
/// `PlayTransport` 枚举.
#[derive(Clone)]
pub enum PlayTransport {
    /// `TcpInterleaved` variant.
    /// `TcpInterleaved` 变体.
    TcpInterleaved { rtp_channel: u8, rtcp_channel: u8 },
    /// `UdpUnicast` variant.
    /// `UdpUnicast` 变体.
    UdpUnicast {
        rtp_socket: Arc<dyn AsyncUdpSocket>,
        rtcp_socket: Arc<dyn AsyncUdpSocket>,
        target_rtp: SocketAddr,
        target_rtcp: SocketAddr,
    },
    /// `UdpMulticast` variant.
    /// `UdpMulticast` 变体.
    UdpMulticast {
        rtp_socket: Arc<dyn AsyncUdpSocket>,
        rtcp_socket: Arc<dyn AsyncUdpSocket>,
        target_rtp: SocketAddr,
        target_rtcp: SocketAddr,
        stream_key: StreamKey,
        track_id: TrackId,
    },
}

/// `PlayTrackState` data structure.
/// `PlayTrackState` 数据结构.
#[derive(Clone)]
pub struct PlayTrackState {
    /// `transport` field of type `PlayTransport`.
    /// `transport` 字段，类型为 `PlayTransport`.
    pub transport: PlayTransport,
    /// `payload_type` field of type `u8`.
    /// `payload_type` 字段，类型为 `u8`.
    pub payload_type: u8,
    /// `seq` field of type `u16`.
    /// `seq` 字段，类型为 `u16`.
    pub seq: u16,
    /// `ssrc` field of type `u32`.
    /// `ssrc` 字段，类型为 `u32`.
    pub ssrc: u32,
    /// `packets_sent` field of type `u32`.
    /// `packets_sent` 字段，类型为 `u32`.
    pub packets_sent: u32,
    /// `octets_sent` field of type `u32`.
    /// `octets_sent` 字段，类型为 `u32`.
    pub octets_sent: u32,
    /// `last_rtp_timestamp` field of type `u32`.
    /// `last_rtp_timestamp` 字段，类型为 `u32`.
    pub last_rtp_timestamp: u32,
    /// `timestamp_repair_count` field of type `u64`.
    /// `timestamp_repair_count` 字段，类型为 `u64`.
    pub timestamp_repair_count: u64,
    /// `sdes_sent` field of type `bool`.
    /// `sdes_sent` 字段，类型为 `bool`.
    pub sdes_sent: bool,
    /// `first_raw_timestamp` field.
    /// `first_raw_timestamp` 字段.
    pub first_raw_timestamp: Option<u32>,
}

/// `PlaySession` data structure.
/// `PlaySession` 数据结构.
pub struct PlaySession {
    /// `cancel` field of type `CancellationToken`.
    /// `cancel` 字段，类型为 `CancellationToken`.
    pub cancel: CancellationToken,
    /// `join` field.
    /// `join` 字段.
    pub join: Box<dyn RuntimeJoinHandle>,
}

/// `PublishUdpTrack` data structure.
/// `PublishUdpTrack` 数据结构.
#[derive(Clone)]
pub struct PublishUdpTrack {
    /// `rtp_socket` field.
    /// `rtp_socket` 字段.
    pub rtp_socket: Arc<dyn AsyncUdpSocket>,
    /// `rtcp_socket` field.
    /// `rtcp_socket` 字段.
    pub rtcp_socket: Arc<dyn AsyncUdpSocket>,
    /// `target_rtp` field of type `SocketAddr`.
    /// `target_rtp` 字段，类型为 `SocketAddr`.
    pub target_rtp: SocketAddr,
    /// `target_rtcp` field of type `SocketAddr`.
    /// `target_rtcp` 字段，类型为 `SocketAddr`.
    pub target_rtcp: SocketAddr,
}

/// `RtspConnectionState` data structure.
/// `RtspConnectionState` 数据结构.
pub struct RtspConnectionState {
    /// `session_id` field of type `String`.
    /// `session_id` 字段，类型为 `String`.
    pub session_id: String,
    /// `peer_addr` field.
    /// `peer_addr` 字段.
    pub peer_addr: Option<SocketAddr>,
    /// `describe_pending` field.
    /// `describe_pending` 字段.
    pub describe_pending: Option<CancellationToken>,
    /// `describe_base_uri` field.
    /// `describe_base_uri` 字段.
    pub describe_base_uri: Option<String>,
    /// `play_response_range` field.
    /// `play_response_range` 字段.
    pub play_response_range: Option<String>,
    /// `stream_key` field.
    /// `stream_key` 字段.
    pub stream_key: Option<StreamKey>,
    /// `mode` field.
    /// `mode` 字段.
    pub mode: Option<SessionMode>,
    /// `announced_tracks` field.
    /// `announced_tracks` 字段.
    pub announced_tracks: HashMap<TrackId, TrackInfo>,
    /// `announced_control_to_track` field.
    /// `announced_control_to_track` 字段.
    pub announced_control_to_track: HashMap<String, TrackId>,
    /// `describe_tracks` field.
    /// `describe_tracks` 字段.
    pub describe_tracks: Vec<TrackInfo>,
    /// `describe_control_to_track` field.
    /// `describe_control_to_track` 字段.
    pub describe_control_to_track: HashMap<String, TrackId>,
    /// `auth_digest_nonce` field.
    /// `auth_digest_nonce` 字段.
    pub auth_digest_nonce: Option<String>,
    /// `auth_digest_nonce_issued_at_micros` field.
    /// `auth_digest_nonce_issued_at_micros` 字段.
    pub auth_digest_nonce_issued_at_micros: Option<u64>,
    /// `auth_digest_nc_last` field of type `u32`.
    /// `auth_digest_nc_last` 字段，类型为 `u32`.
    pub auth_digest_nc_last: u32,
    /// `publish` field.
    /// `publish` 字段.
    pub publish: Option<PublishSession>,
    /// `play` field.
    /// `play` 字段.
    pub play: Option<PlaySession>,
    /// `play_tracks` field.
    /// `play_tracks` 字段.
    pub play_tracks: HashMap<TrackId, PlayTrackState>,
}

impl RtspConnectionState {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(connection_id: RtspConnectionId) -> Self {
        Self {
            session_id: format!("rtsp-{connection_id}"),
            peer_addr: None,
            describe_pending: None,
            describe_base_uri: None,
            play_response_range: None,
            stream_key: None,
            mode: None,
            announced_tracks: HashMap::new(),
            announced_control_to_track: HashMap::new(),
            describe_tracks: Vec::new(),
            describe_control_to_track: HashMap::new(),
            auth_digest_nonce: None,
            auth_digest_nonce_issued_at_micros: None,
            auth_digest_nc_last: 0,
            publish: None,
            play: None,
            play_tracks: HashMap::new(),
        }
    }
}
