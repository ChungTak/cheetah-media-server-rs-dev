use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use cheetah_codec::{MuteAudioMaker, ParameterSetCache, TimestampNormalizer, TrackId, TrackInfo};
use cheetah_rtsp_driver_tokio::RtspConnectionId;
use cheetah_sdk::{
    AsyncUdpSocket, CancellationToken, JoinHandle as RuntimeJoinHandle, PublishLease,
    PublisherSink, StreamKey,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Publish,
    Play,
}

#[derive(Default)]
pub struct PublishTrackClock {
    pub base_seq: Option<u16>,
    pub max_seq: u16,
    pub cycles: u32,
    pub received_packets: u32,
    pub jitter: u32,
    pub last_transit: Option<i64>,
    pub last_sr_lsr: Option<u32>,
    pub last_sr_unix_micros: Option<u64>,
    pub last_rr_expected: u32,
    pub last_rr_received: u32,
}

#[derive(Default)]
pub struct PublishH264Depacketizer {
    pub fu_buffer: Vec<u8>,
    pub access_unit: Vec<u8>,
    pub access_unit_timestamp: Option<u32>,
    pub access_unit_last_sequence: Option<u16>,
    pub access_unit_keyframe: bool,
    pub access_unit_marker_seen: bool,
}

#[derive(Default)]
pub struct PublishH265Depacketizer {
    pub fu_buffer: Vec<u8>,
    pub access_unit: Vec<u8>,
    pub access_unit_timestamp: Option<u32>,
    pub access_unit_last_sequence: Option<u16>,
    pub access_unit_keyframe: bool,
    pub access_unit_marker_seen: bool,
}

#[derive(Default)]
pub struct PublishAv1Depacketizer {
    pub access_unit: Vec<u8>,
    pub current_obu: Vec<u8>,
    pub access_unit_timestamp: Option<u32>,
    pub access_unit_last_sequence: Option<u16>,
    pub access_unit_keyframe: bool,
    pub access_unit_marker_seen: bool,
}

#[derive(Default)]
pub struct PublishVp9Depacketizer {
    pub access_unit: Vec<u8>,
    pub access_unit_timestamp: Option<u32>,
    pub access_unit_last_sequence: Option<u16>,
    pub access_unit_keyframe: bool,
}

#[derive(Default)]
pub struct PublishVp8Depacketizer {
    pub access_unit: Vec<u8>,
    pub access_unit_timestamp: Option<u32>,
    pub access_unit_last_sequence: Option<u16>,
    pub access_unit_keyframe: bool,
}

pub struct PublishTrackTimestampState {
    pub normalizer: TimestampNormalizer,
    pub repair_count: u64,
    pub source_disorder_count: u64,
    pub discontinuity_count: u64,
}

pub struct RtcpReceiverMetrics {
    pub fraction_lost: u8,
    pub cumulative_lost: u32,
    pub extended_highest_seq: u32,
    pub jitter: u32,
    pub lsr: u32,
    pub dlsr: u32,
}

impl PublishTrackClock {
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

    pub fn note_sender_report(&mut self, lsr: u32, arrival_unix_micros: u64) {
        self.last_sr_lsr = Some(lsr);
        self.last_sr_unix_micros = Some(arrival_unix_micros);
    }

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

pub struct PublishSession {
    pub cancel: CancellationToken,
    pub lease: PublishLease,
    pub sink: Box<dyn PublisherSink>,
    pub record_started: bool,
    pub pre_record_rtp_drop_count: u64,
    pub timestamp_repair_alert_threshold: u64,
    pub queue_drop_alert_threshold: u64,
    pub queue_drop_counts: HashMap<TrackId, u64>,
    pub unsupported_codec_drop_counts: HashMap<TrackId, u64>,
    pub compat_probe_drop_counts: HashMap<TrackId, u64>,
    pub tracks: HashMap<TrackId, TrackInfo>,
    pub track_channels: HashMap<u8, TrackId>,
    pub rtcp_channels: HashMap<u8, TrackId>,
    pub clocks: HashMap<TrackId, PublishTrackClock>,
    pub h264_depacketizers: HashMap<TrackId, PublishH264Depacketizer>,
    pub h265_depacketizers: HashMap<TrackId, PublishH265Depacketizer>,
    pub av1_depacketizers: HashMap<TrackId, PublishAv1Depacketizer>,
    pub vp9_depacketizers: HashMap<TrackId, PublishVp9Depacketizer>,
    pub vp8_depacketizers: HashMap<TrackId, PublishVp8Depacketizer>,
    pub track_last_frame_timestamps: HashMap<TrackId, i64>,
    pub timestamp_normalizers: HashMap<TrackId, PublishTrackTimestampState>,
    pub video_parameter_sets: HashMap<TrackId, ParameterSetCache>,
    pub udp_tracks: HashMap<TrackId, PublishUdpTrack>,
    pub udp_task_handles: Vec<Box<dyn RuntimeJoinHandle>>,
    pub mute_audio_maker: Option<MuteAudioMaker>,
    pub codec_probed: HashSet<TrackId>,
}

#[derive(Clone)]
pub enum PlayTransport {
    TcpInterleaved {
        rtp_channel: u8,
        rtcp_channel: u8,
    },
    UdpUnicast {
        rtp_socket: Arc<dyn AsyncUdpSocket>,
        rtcp_socket: Arc<dyn AsyncUdpSocket>,
        target_rtp: SocketAddr,
        target_rtcp: SocketAddr,
    },
    UdpMulticast {
        rtp_socket: Arc<dyn AsyncUdpSocket>,
        rtcp_socket: Arc<dyn AsyncUdpSocket>,
        target_rtp: SocketAddr,
        target_rtcp: SocketAddr,
        stream_key: StreamKey,
        track_id: TrackId,
    },
}

#[derive(Clone)]
pub struct PlayTrackState {
    pub transport: PlayTransport,
    pub payload_type: u8,
    pub seq: u16,
    pub ssrc: u32,
    pub packets_sent: u32,
    pub octets_sent: u32,
    pub last_rtp_timestamp: u32,
    pub timestamp_repair_count: u64,
    pub sdes_sent: bool,
    pub first_raw_timestamp: Option<u32>,
}

pub struct PlaySession {
    pub cancel: CancellationToken,
    pub join: Box<dyn RuntimeJoinHandle>,
}

#[derive(Clone)]
pub struct PublishUdpTrack {
    pub rtp_socket: Arc<dyn AsyncUdpSocket>,
    pub rtcp_socket: Arc<dyn AsyncUdpSocket>,
    pub target_rtp: SocketAddr,
    pub target_rtcp: SocketAddr,
}

pub struct RtspConnectionState {
    pub session_id: String,
    pub peer_addr: Option<SocketAddr>,
    pub describe_pending: Option<CancellationToken>,
    pub describe_base_uri: Option<String>,
    pub play_response_range: Option<String>,
    pub stream_key: Option<StreamKey>,
    pub mode: Option<SessionMode>,
    pub announced_tracks: HashMap<TrackId, TrackInfo>,
    pub announced_control_to_track: HashMap<String, TrackId>,
    pub describe_tracks: Vec<TrackInfo>,
    pub describe_control_to_track: HashMap<String, TrackId>,
    pub auth_digest_nonce: Option<String>,
    pub auth_digest_nonce_issued_at_micros: Option<u64>,
    pub auth_digest_nc_last: u32,
    pub publish: Option<PublishSession>,
    pub play: Option<PlaySession>,
    pub play_tracks: HashMap<TrackId, PlayTrackState>,
}

impl RtspConnectionState {
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
