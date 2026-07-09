use bytes::Bytes;
use cheetah_codec::{
    depacketize_payload, packetize_payload, AVFrame, AccessUnit, CodecConfigPayload,
    CodecConfigRequirement, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind,
    ParameterSetCache, ParameterSetRequirement, RtpHeader, Timebase, TimestampAlert,
    TimestampNormalizeInput, TimestampNormalizeMode, TimestampNormalizer,
    TimestampNormalizerConfig, TimestampValue, TrackId, TrackInfo,
};

#[test]
fn timestamp_matrix_covers_wrap_repeat_negative_cts_and_reset() {
    let mut normalizer = TimestampNormalizer::new(
        TimestampNormalizerConfig::new(Timebase::new(1, 90_000), Timebase::new(1, 1_000), Some(16))
            .expect("valid")
            .with_negative_composition_allowed(false),
    );

    let first = normalizer
        .normalize(TimestampNormalizeInput {
            mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                dts: TimestampValue::Wrapped((u16::MAX - 3) as u64),
                composition_offset: None,
            },
            frame_duration: None,
            fallback_step: None,
            is_video: true,
            force_discontinuity: false,
        })
        .expect("first");
    let second = normalizer
        .normalize(TimestampNormalizeInput {
            mode: TimestampNormalizeMode::DtsPts {
                dts: TimestampValue::Wrapped(12),
                pts: TimestampValue::Wrapped(8),
            },
            frame_duration: None,
            fallback_step: None,
            is_video: true,
            force_discontinuity: false,
        })
        .expect("second");
    assert!(second.dts > first.dts);
    assert_eq!(second.pts, second.dts);
    assert!(second
        .alerts
        .contains(&TimestampAlert::NegativeCompositionClamped));

    let third = normalizer
        .normalize(TimestampNormalizeInput {
            mode: TimestampNormalizeMode::DtsPts {
                dts: TimestampValue::Unwrapped(second.dts),
                pts: TimestampValue::Unwrapped(second.dts),
            },
            frame_duration: None,
            fallback_step: None,
            is_video: true,
            force_discontinuity: false,
        })
        .expect("third");
    assert!(!third.discontinuity);
    assert!(third
        .alerts
        .contains(&TimestampAlert::NonMonotonicDtsRepaired));

    normalizer.reset();
    let after_reset = normalizer
        .normalize(TimestampNormalizeInput {
            mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                dts: TimestampValue::Unwrapped(5),
                composition_offset: None,
            },
            frame_duration: None,
            fallback_step: None,
            is_video: false,
            force_discontinuity: false,
        })
        .expect("after reset");
    assert!(after_reset.discontinuity);
    assert!(after_reset.alerts.contains(&TimestampAlert::ResetApplied));
}

#[test]
fn timestamp_matrix_pts_only_bframe_reorder_keeps_monotonic_dts_for_all_video_codecs() {
    let video_codecs = [
        CodecId::H264,
        CodecId::H265,
        CodecId::H266,
        CodecId::AV1,
        CodecId::VP8,
        CodecId::VP9,
    ];
    for codec in video_codecs {
        let mut normalizer = TimestampNormalizer::new(
            TimestampNormalizerConfig::new(
                Timebase::new(1, 90_000),
                Timebase::new(1, 90_000),
                Some(32),
            )
            .expect("valid config"),
        );
        let pts_in_decode_arrival_order = [0_i64, 9_000, 3_000, 6_000, 12_000];
        let mut out = Vec::new();
        for pts in pts_in_decode_arrival_order {
            out.push(
                normalizer
                    .normalize(TimestampNormalizeInput {
                        mode: TimestampNormalizeMode::PtsOnly {
                            pts: TimestampValue::Unwrapped(pts),
                        },
                        frame_duration: Some(3_000),
                        fallback_step: Some(3_000),
                        is_video: true,
                        force_discontinuity: false,
                    })
                    .expect("pts-only normalized"),
            );
        }

        let mut prev_dts = i64::MIN;
        let mut saw_reorder_alert = false;
        for item in &out {
            assert!(item.dts > prev_dts, "{codec:?} dts must stay monotonic");
            saw_reorder_alert |= item.alerts.contains(&TimestampAlert::PtsReorderObserved);
            assert!(
                !item.discontinuity,
                "{codec:?} reorder path should not force discontinuity"
            );
            prev_dts = item.dts;
        }
        assert!(
            saw_reorder_alert,
            "{codec:?} should surface PtsReorderObserved on small backward reorder"
        );
    }
}

#[test]
fn timestamp_matrix_pts_only_non_bframe_stays_stable_for_all_video_codecs() {
    let video_codecs = [
        CodecId::H264,
        CodecId::H265,
        CodecId::H266,
        CodecId::AV1,
        CodecId::VP8,
        CodecId::VP9,
    ];
    for codec in video_codecs {
        let mut normalizer = TimestampNormalizer::new(
            TimestampNormalizerConfig::new(
                Timebase::new(1, 90_000),
                Timebase::new(1, 90_000),
                Some(32),
            )
            .expect("valid config"),
        );
        let pts_sequence = [0_i64, 3_000, 6_000, 9_000, 12_000];
        let mut prev_dts = i64::MIN;
        let mut saw_reorder_alert = false;
        for pts in pts_sequence {
            let normalized = normalizer
                .normalize(TimestampNormalizeInput {
                    mode: TimestampNormalizeMode::PtsOnly {
                        pts: TimestampValue::Unwrapped(pts),
                    },
                    frame_duration: None,
                    fallback_step: Some(3_000),
                    is_video: true,
                    force_discontinuity: false,
                })
                .expect("pts-only normalized");
            if prev_dts != i64::MIN {
                let step = normalized.dts - prev_dts;
                assert!(
                    (2_990..=3_010).contains(&step),
                    "{codec:?} expected stable ~3000 tick dts step, got {step}"
                );
            }
            saw_reorder_alert |= normalized
                .alerts
                .contains(&TimestampAlert::PtsReorderObserved);
            prev_dts = normalized.dts;
        }
        assert!(
            !saw_reorder_alert,
            "{codec:?} no-b-frame path must not emit reorder alert"
        );
    }
}

#[test]
fn timestamp_matrix_rtmp_dts_cts_and_reset_edge_cases_cover_video_and_audio_codecs() {
    let start = u32::MAX - 2_000;
    let video_codecs = [
        CodecId::H264,
        CodecId::H265,
        CodecId::H266,
        CodecId::AV1,
        CodecId::VP8,
        CodecId::VP9,
    ];
    for codec in video_codecs {
        let mut normalizer = TimestampNormalizer::new(
            TimestampNormalizerConfig::new(
                Timebase::new(1, 90_000),
                Timebase::new(1, 90_000),
                Some(32),
            )
            .expect("valid config"),
        );
        let outputs = [
            TimestampValue::Wrapped(u64::from(start)),
            TimestampValue::Wrapped(u64::from(start.wrapping_add(3_000))),
            TimestampValue::Wrapped(u64::from(start.wrapping_add(3_000))),
            TimestampValue::Wrapped(u64::from(start.wrapping_add(2_900))),
            TimestampValue::Unwrapped(i64::from(start) + 120_000_000),
        ]
        .into_iter()
        .map(|dts| {
            normalizer.normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                    dts,
                    composition_offset: Some(1_800),
                },
                frame_duration: None,
                fallback_step: Some(3_000),
                is_video: true,
                force_discontinuity: false,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .expect("video normalization");

        assert_eq!(outputs[0].dts, 0, "{codec:?} must remove random epoch");
        assert!(
            outputs[1].dts >= 3_000,
            "{codec:?} wrap progression must move forward"
        );
        assert!(outputs[2]
            .alerts
            .contains(&TimestampAlert::NonMonotonicDtsRepaired));
        assert!(outputs[3]
            .alerts
            .contains(&TimestampAlert::NonMonotonicDtsRepaired));
        assert!(
            outputs[4].discontinuity,
            "{codec:?} big jump should mark discontinuity"
        );
        assert!(outputs[4]
            .alerts
            .contains(&TimestampAlert::TimelineDiscontinuityDetected));
    }

    let audio_codecs = [
        CodecId::AAC,
        CodecId::Opus,
        CodecId::G711A,
        CodecId::G711U,
        CodecId::MP3,
    ];
    for codec in audio_codecs {
        let mut normalizer = TimestampNormalizer::new(
            TimestampNormalizerConfig::new(
                Timebase::new(1, 1_000),
                Timebase::new(1, 1_000),
                Some(32),
            )
            .expect("valid config"),
        );
        let outputs = [
            TimestampValue::Wrapped(u64::from(start)),
            TimestampValue::Wrapped(u64::from(start.wrapping_add(40))),
            TimestampValue::Wrapped(u64::from(start.wrapping_add(40))),
            TimestampValue::Wrapped(u64::from(start.wrapping_add(39))),
            TimestampValue::Unwrapped(i64::from(start) + 3_000_000),
        ]
        .into_iter()
        .map(|dts| {
            normalizer.normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                    dts,
                    composition_offset: None,
                },
                frame_duration: Some(40),
                fallback_step: Some(40),
                is_video: false,
                force_discontinuity: false,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .expect("audio normalization");

        for item in &outputs {
            assert_eq!(
                item.pts, item.dts,
                "{codec:?} audio pts must align with dts"
            );
        }
        assert!(outputs[2]
            .alerts
            .contains(&TimestampAlert::NonMonotonicDtsRepaired));
        assert!(outputs[3]
            .alerts
            .contains(&TimestampAlert::NonMonotonicDtsRepaired));
        assert!(
            outputs[4].discontinuity,
            "{codec:?} big jump should mark discontinuity"
        );
    }
}

#[test]
fn access_unit_matrix_covers_bframe_and_non_bframe_timing_rules() {
    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        100,
        110,
        Timebase::new(1, 90_000),
        Bytes::from_static(&[0x65, 0x00]),
    );

    let cache = ParameterSetCache::default();
    let err = AccessUnit::from_frame_units(&frame, vec![Bytes::from_static(&[0x65, 0x00])], &cache)
        .expect_err("non B-frame with pts < dts must fail");
    let err_text = err.to_string();
    assert!(err_text.contains("pts < dts"));

    frame.flags.insert(FrameFlags::B_FRAME);
    let au = AccessUnit::from_frame_units(&frame, vec![Bytes::from_static(&[0x41, 0x01])], &cache)
        .expect("b-frame should be accepted");
    assert!(!au.random_access);
    assert!(matches!(
        au.parameter_set_requirement,
        ParameterSetRequirement::NotRequired
    ));
}

#[test]
fn rtp_matrix_covers_multi_packet_out_of_order_and_marker_noise() {
    let payload = (0..96u8).collect::<Vec<_>>();
    let mut packets = packetize_payload(
        &payload,
        28,
        RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 10,
            timestamp: 12345,
            ssrc: 77,
            marker: false,
        },
    );
    assert!(packets.len() > 2);

    packets[0].header.marker = true;
    for packet in packets.iter_mut().skip(1) {
        packet.header.marker = false;
    }
    packets.reverse();

    let rebuilt = depacketize_payload(packets);
    assert_eq!(rebuilt, Bytes::from(payload));
}

#[test]
fn parameter_set_matrix_covers_late_arrival_and_rotation() {
    let mut cache = ParameterSetCache::default();
    assert!(matches!(
        cache.requirement_for_frame(CodecId::H264, true),
        ParameterSetRequirement::RequiredMissing
    ));

    let first_sets = [
        0, 0, 0, 1, 0x67, 0x64, 0x00, 0x1f, 0, 0, 0, 1, 0x68, 0xeb, 0xe3, 0xcb,
    ];
    assert!(cache.update_from_annexb(CodecId::H264, &first_sets));
    assert!(matches!(
        cache.requirement_for_frame(CodecId::H264, true),
        ParameterSetRequirement::RequiredPresent
    ));

    let rotated_sps = [0, 0, 0, 1, 0x67, 0x42, 0x00, 0x2a];
    assert!(cache.update_from_annexb(CodecId::H264, &rotated_sps));
    assert_eq!(cache.sps.as_deref(), Some(&[0x67, 0x42, 0x00, 0x2a][..]));
}

#[test]
fn codec_config_video_matrix_covers_h26x_av1_vp8_vp9() {
    let mut h264 = TrackInfo::new(TrackId(11), MediaKind::Video, CodecId::H264, 90_000);
    h264.extradata = CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x01])],
        pps: vec![Bytes::from_static(&[0x68, 0x01])],
        avcc: None,
    };

    let mut h265 = TrackInfo::new(TrackId(12), MediaKind::Video, CodecId::H265, 90_000);
    h265.extradata = CodecExtradata::H265 {
        vps: vec![Bytes::from_static(&[0x40, 0x01])],
        sps: vec![Bytes::from_static(&[0x42, 0x01])],
        pps: vec![Bytes::from_static(&[0x44, 0x01])],
        hvcc: None,
    };

    let mut h266 = TrackInfo::new(TrackId(13), MediaKind::Video, CodecId::H266, 90_000);
    h266.extradata = CodecExtradata::H266 {
        vps: vec![Bytes::from_static(&[0x00, 0x70, 0x01])],
        sps: vec![Bytes::from_static(&[0x00, 0x78, 0x01])],
        pps: vec![Bytes::from_static(&[0x00, 0x80, 0x01])],
    };

    let mut av1 = TrackInfo::new(TrackId(14), MediaKind::Video, CodecId::AV1, 90_000);
    av1.extradata = CodecExtradata::AV1 {
        sequence_header: Some(Bytes::from_static(&[0x81, 0x00])),
        codec_config: None,
    };

    let mut vp8 = TrackInfo::new(TrackId(15), MediaKind::Video, CodecId::VP8, 90_000);
    vp8.extradata = CodecExtradata::VP8 {
        config: Some(Bytes::from_static(&[0x10])),
    };

    let mut vp9 = TrackInfo::new(TrackId(16), MediaKind::Video, CodecId::VP9, 90_000);
    vp9.extradata = CodecExtradata::VP9 {
        config: Some(Bytes::from_static(&[0x20])),
    };

    let tracks = [h264, h265, h266, av1, vp8, vp9];
    for track in tracks {
        let view = track.codec_config_view().expect("config view");
        match track.codec {
            CodecId::H264 | CodecId::H265 | CodecId::H266 => {
                assert!(matches!(view.requirement, CodecConfigRequirement::Required));
            }
            CodecId::AV1 | CodecId::VP8 | CodecId::VP9 => {
                assert!(matches!(view.requirement, CodecConfigRequirement::Optional));
            }
            _ => unreachable!("unexpected video codec in test matrix"),
        }
    }
}

#[test]
fn codec_config_audio_matrix_covers_aac_opus_g711_mp3() {
    let mut aac = TrackInfo::new(TrackId(21), MediaKind::Audio, CodecId::AAC, 48_000);
    aac.extradata = CodecExtradata::AAC {
        asc: Bytes::from_static(&[0x12, 0x10]),
    };

    let mut opus = TrackInfo::new(TrackId(22), MediaKind::Audio, CodecId::Opus, 48_000);
    opus.extradata = CodecExtradata::Opus {
        fmtp: Some("sprop-stereo=1".to_string()),
        channel_mapping: None,
    };

    let g711a = TrackInfo::new(TrackId(23), MediaKind::Audio, CodecId::G711A, 8_000);
    let g711u = TrackInfo::new(TrackId(24), MediaKind::Audio, CodecId::G711U, 8_000);

    let mut mp3 = TrackInfo::new(TrackId(25), MediaKind::Audio, CodecId::MP3, 44_100);
    mp3.extradata = CodecExtradata::MP3 {
        side_info: Some(Bytes::from_static(&[0x7f])),
    };

    let aac_view = aac.codec_config_view().expect("aac view");
    assert!(matches!(
        aac_view.requirement,
        CodecConfigRequirement::Required
    ));

    let opus_view = opus.codec_config_view().expect("opus view");
    assert!(matches!(
        opus_view.requirement,
        CodecConfigRequirement::Optional
    ));

    let mp3_view = mp3.codec_config_view().expect("mp3 view");
    assert!(matches!(
        mp3_view.requirement,
        CodecConfigRequirement::Optional
    ));

    let g711a_view = g711a.codec_config_view().expect("g711a view");
    let g711u_view = g711u.codec_config_view().expect("g711u view");
    assert!(matches!(
        g711a_view.requirement,
        CodecConfigRequirement::None
    ));
    assert!(matches!(
        g711u_view.requirement,
        CodecConfigRequirement::None
    ));
    assert!(matches!(g711a_view.payload, CodecConfigPayload::None));
    assert!(matches!(g711u_view.payload, CodecConfigPayload::None));
}
