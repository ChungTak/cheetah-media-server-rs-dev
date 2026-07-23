use super::*;
use cheetah_codec::{FrameFlags, PsMuxer, TrackReadiness};

/// Create a small PS-muxed RTP payload carrying a single H264 keyframe with the given PTS.
fn ps_payload_with_pts(pts: i64) -> Bytes {
    let mut muxer = PsMuxer::new();
    let mut track = TrackInfo::new(TrackId(0xE0), MediaKind::Video, CodecId::H264, 90_000);
    track.readiness = TrackReadiness::Ready;
    muxer.add_track(track);

    // Annex-B keyframe with SPS start code so the PS demuxer recognizes a video frame.
    let mut payload = vec![0, 0, 0, 1, 0x67, 0x42, 0, 0x0A];
    payload.extend_from_slice(b"frame");
    let mut frame = AVFrame::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        pts,
        pts,
        Timebase::new(1, 90_000),
        Bytes::from(payload),
    );
    frame.flags.insert(FrameFlags::KEY);

    muxer.mux(&frame).expect("mux PS frame")
}

/// Build an `RtpPacket` carrying a PS payload for the given SSRC / seq / pts.
fn rtp_packet(ssrc: u32, seq: u16, ts: u32, payload: Bytes) -> RtpPacket {
    RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: seq,
            timestamp: ts,
            ssrc,
            marker: false,
        },
        payload,
    }
}

#[test]
fn test_rtp_core_reorders_out_of_order_ps_packets() {
    let mut core = RtpCore::new(8, 30_000);

    let session_key = "test/recv".to_string();
    let ssrc = 0x1234_5678;
    let source: SocketAddr = "127.0.0.1:5000".parse().unwrap();

    // Create a receiving server with explicit PS mode.
    let _outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: session_key.clone(),
            ssrc: Some(ssrc),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    // The PS demuxer emits a frame one pack late, so feed four packets and expect
    // three frames. Arrival order is seq 1, 3, 2, 4. PTS follows the sequence number:
    // seq 1 -> pts 1000, seq 2 -> pts 2000, seq 3 -> pts 3000, seq 4 -> pts 4000.
    // After reorder the core processes seq 1, 2, 3, 4 in order; the demuxer therefore
    // emits frames for pts 1000, 2000, 3000 in order.
    let arrivals = [(1, 1_000), (3, 3_000), (2, 2_000), (4, 4_000)];

    let mut all_frames = Vec::new();
    for (seq, pts) in arrivals {
        let pkt = rtp_packet(ssrc, seq, 0, ps_payload_with_pts(pts));
        let datagram = RtpDatagram {
            source,
            data: pkt.encode(),
            received_at_ms: seq as u64 * 10,
        };
        let outputs = core.handle_input(RtpCoreInput::UdpPacket(datagram));
        for o in outputs {
            if let RtpCoreOutput::Event(RtpCoreEvent::Frame { frame, .. }) = o {
                all_frames.push(frame);
            }
        }
    }

    let pts_values: Vec<_> = all_frames.iter().map(|f| f.pts).collect();
    assert_eq!(
        pts_values,
        vec![999, 1_999, 2_999],
        "frames must be delivered in sequence order; got {pts_values:?}"
    );
}

#[test]
fn test_rtp_core_duplicate_packet_is_suppressed() {
    let mut core = RtpCore::new(8, 30_000);

    let session_key = "test/dup".to_string();
    let ssrc = 0x1234_5678;
    let source: SocketAddr = "127.0.0.1:5000".parse().unwrap();

    let _outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: session_key.clone(),
            ssrc: Some(ssrc),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let payload = ps_payload_with_pts(10_000);
    let mut track_found = 0;
    let mut frame_count = 0;

    // Feed seq 1, then a duplicate seq 1, then seq 2. The duplicate must not cause
    // an extra TrackInfo or an extra frame.
    let sequences = [1, 1, 2];
    for seq in sequences {
        let pkt = rtp_packet(ssrc, seq, 0, payload.clone());
        let datagram = RtpDatagram {
            source,
            data: pkt.encode(),
            received_at_ms: 10,
        };
        let outputs = core.handle_input(RtpCoreInput::UdpPacket(datagram));
        for o in outputs {
            match o {
                RtpCoreOutput::Event(RtpCoreEvent::TrackFound { .. }) => track_found += 1,
                RtpCoreOutput::Event(RtpCoreEvent::Frame { .. }) => frame_count += 1,
                _ => {}
            }
        }
    }

    assert_eq!(track_found, 1, "duplicate must not create a second track");
    assert_eq!(frame_count, 1, "duplicate RTP packet must be suppressed");
}
