use super::*;
use cheetah_codec::{FrameFlags, PsMuxer, TrackReadiness};

fn ps_payload(pts: i64) -> Bytes {
    let mut muxer = PsMuxer::new();
    let mut track = TrackInfo::new(TrackId(0xE0), MediaKind::Video, CodecId::H264, 90_000);
    track.readiness = TrackReadiness::Ready;
    muxer.add_track(track);

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

#[test]
fn test_format_changed_on_resolvable_pt_switch() {
    let mut core = RtpCore::new(10, 30_000);

    // Lock the session to Es (H.264 Annex-B) on PT 96.
    for seq in 1..=2u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 4000,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    }

    let session = core
        .sessions
        .get("live/4000")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::Es);

    // A mid-stream switch to static PT 33 (MP2T) with a TS sync byte is resolvable
    // and should emit a FormatChanged event.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 33,
            sequence_number: 3,
            timestamp: 3,
            ssrc: 4000,
            marker: false,
        },
        payload: Bytes::from(vec![0x47, 0x00, 0x01, 0x10]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));

    let changed = outputs.iter().any(|o| {
        matches!(
            o,
            RtpCoreOutput::Event(RtpCoreEvent::FormatChanged {
                payload_type: 33,
                old_payload_mode: RtpPayloadMode::Es,
                new_payload_mode: RtpPayloadMode::Ts,
                ..
            })
        )
    });
    assert!(changed, "expected FormatChanged on PT switch");

    let session = core.sessions.get("live/4000").expect("session still alive");
    assert_eq!(session.payload_mode, RtpPayloadMode::Ts);
}

#[test]
fn test_session_closed_on_oscillating_pt_modes() {
    let mut core = RtpCore::new(10, 30_000);

    // Lock the session to RawAudio on static PT 0.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 0,
            sequence_number: 1,
            timestamp: 1,
            ssrc: 4100,
            marker: false,
        },
        payload: Bytes::from(vec![0x00]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));

    let session = core
        .sessions
        .get("live/4100")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::RawAudio);

    // Oscillate between PT 33 (Ts) and PT 0 (RawAudio). Each switch increments the
    // format-change budget. The fourth mode switch exceeds the default budget and closes
    // the session instead of emitting another FormatChanged.
    let mut seq = 2u16;
    let pts = [33u8, 0, 33, 0];
    let mut final_outputs = Vec::new();
    for (i, pt) in pts.iter().enumerate() {
        let payload = if *pt == 33 {
            vec![0x47, 0x00, 0x01, 0x10]
        } else {
            vec![0x00]
        };
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: *pt,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 4100,
                marker: false,
            },
            payload: Bytes::from(payload),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        final_outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
        seq += 1;

        // First three switches should keep the session alive.
        if i < 3 {
            assert!(
                core.sessions.contains_key("live/4100"),
                "session should survive {} format switches",
                i + 1
            );
        }
    }

    assert!(
        !core.sessions.contains_key("live/4100"),
        "session should be closed after repeated mode oscillation"
    );
    assert!(final_outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::CloseSession(key) if key == "live/4100"
    )));
}

#[test]
fn test_session_closed_on_unresolvable_pt_switch() {
    let mut core = RtpCore::new(10, 30_000);
    // Keep the close threshold small for this test; the default is much larger to
    // tolerate legitimate DTMF/FEC/RED bursts.
    core.set_max_tolerated_unknown_pt_packets(8);

    // Lock the session to Es on PT 96.
    for seq in 1..=2u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 5000,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    }

    let session = core
        .sessions
        .get("live/5000")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::Es);

    // A persistent run of unresolvable PT packets (matching the probe budget) closes
    // the session; short DTMF/FEC bursts are tolerated.
    for seq in 3..=10u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 97,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 5000,
                marker: false,
            },
            payload: Bytes::from(vec![0xAB, 0xCD]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let outputs = if seq == 10 {
            core.handle_input(RtpCoreInput::UdpPacket(dgram))
        } else {
            let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
            Vec::new()
        };

        if seq == 10 {
            let closed = outputs.iter().any(|o| {
                matches!(
                    o,
                    RtpCoreOutput::CloseSession(key) if key == "live/5000"
                )
            });
            assert!(
                closed,
                "expected CloseSession after repeated unresolvable PTs"
            );
            assert!(!core.sessions.contains_key("live/5000"));
        } else {
            assert!(
                core.sessions.contains_key("live/5000"),
                "single unknown PT should be tolerated"
            );
        }
    }
}

#[test]
fn test_interleaved_unknown_pt_is_tolerated() {
    let mut core = RtpCore::new(10, 30_000);

    // Lock the session to Es on PT 96.
    for seq in 1..=2u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 6000,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    }

    // One interleaved unknown PT (RFC 4733 DTMF/FEC) does not close the session.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 97,
            sequence_number: 3,
            timestamp: 3,
            ssrc: 6000,
            marker: false,
        },
        payload: Bytes::from(vec![0xAB, 0xCD]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    assert!(core.sessions.contains_key("live/6000"));

    // Returning to the original PT resumes normal processing.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 4,
            timestamp: 4,
            ssrc: 6000,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    assert!(core.sessions.contains_key("live/6000"));
}

#[test]
fn test_long_unknown_pt_burst_is_tolerated_before_returning_to_locked_pt() {
    let mut core = RtpCore::new(10, 30_000);

    // Lock the session to Es on PT 96.
    for seq in 1..=2u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 6001,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    }

    // A 50-packet DTMF/FEC burst (well below the default 255-packet budget) must not
    // close the session while audio is suspended.
    for seq in 3..=52u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 97,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 6001,
                marker: false,
            },
            payload: Bytes::from(vec![0xAB, 0xCD]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
        assert!(
            !outputs
                .iter()
                .any(|o| matches!(o, RtpCoreOutput::CloseSession(key) if key == "live/6001")),
            "unknown-PT burst should be tolerated"
        );
        assert!(core.sessions.contains_key("live/6001"));
    }

    // Returning to the original PT resumes normal processing.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 53,
            timestamp: 53,
            ssrc: 6001,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    assert!(core.sessions.contains_key("live/6001"));
}

#[test]
fn test_unknown_pt_payload_is_not_fed_to_ps_demuxer() {
    let mut core = RtpCore::new(10, 30_000);

    let session_key = "test/ps-skip".to_string();
    let ssrc = 7000;
    let source: SocketAddr = "127.0.0.1:1".parse().unwrap();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key,
            ssrc: Some(ssrc),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    // Lock the session to PS on PT 96.
    for seq in 1..=2u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc,
                marker: false,
            },
            payload: ps_payload(i64::from(seq) * 1_000),
        };
        let dgram = RtpDatagram {
            source,
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    }

    let session = core.sessions.get("test/ps-skip").expect("created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::Ps);

    // An interleaved unknown PT packet must not be passed to the PS demuxer.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 97,
            sequence_number: 3,
            timestamp: 3,
            ssrc,
            marker: false,
        },
        payload: Bytes::from(vec![0xAB, 0xCD]),
    };
    let dgram = RtpDatagram {
        source,
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    assert!(
        !outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::CloseSession(_))),
        "single unknown PT should be tolerated"
    );
    assert!(core.sessions.contains_key("test/ps-skip"));

    // Resume normal PS packets; the demuxer must still produce the next frame in order,
    // which would not happen if the unknown bytes had been fed into it.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 4,
            timestamp: 4,
            ssrc,
            marker: false,
        },
        payload: ps_payload(4_000),
    };
    let dgram = RtpDatagram {
        source,
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    let frame_pts: Vec<_> = outputs
        .iter()
        .filter_map(|o| match o {
            RtpCoreOutput::Event(RtpCoreEvent::Frame { frame, .. }) => Some(frame.pts),
            _ => None,
        })
        .collect();
    assert!(
        !frame_pts.is_empty(),
        "PS demuxer must continue after an interleaved unknown PT"
    );
}
