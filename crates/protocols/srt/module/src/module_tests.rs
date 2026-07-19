#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SrtAuthUserConfig, SrtFecModuleConfig, SrtPayloadModuleConfig, SrtRelayJobConfig};
    use bytes::Bytes;
    use cheetah_sdk::{HttpMethod, HttpRequest, ModuleFactory};

    #[test]
    fn factory_manifest_matches_srt_module_contract() {
        let manifest = SrtModuleFactory.manifest();
        assert_eq!(manifest.module_id, ModuleId::new("srt"));
        assert_eq!(manifest.config_namespace, "srt");
        assert!(manifest
            .capabilities
            .iter()
            .any(|capability| matches!(capability, ModuleCapability::Publish)));
        assert!(manifest
            .capabilities
            .iter()
            .any(|capability| matches!(capability, ModuleCapability::Subscribe)));
        assert!(manifest
            .capabilities
            .iter()
            .any(|capability| matches!(capability, ModuleCapability::HttpApi)));
    }

    #[test]
    fn metrics_routes_are_registered() {
        let module = SrtModule::new();
        let routes = module.http_routes();

        assert!(routes
            .iter()
            .any(|route| route.method == HttpMethod::Get && route.path == "/metrics"));
        assert!(routes
            .iter()
            .any(|route| route.method == HttpMethod::Get && route.path == "/metrics.json"));
    }

    #[test]
    fn metrics_json_endpoint_starts_at_zero() {
        let module = SrtModule::new();
        let service = module.http_service().expect("SRT HTTP service");
        let response = futures::executor::block_on(service.handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/metrics.json".to_string(),
            query: None,
            headers: Vec::new(),
            body: Default::default(),
        }))
        .expect("metrics.json response");

        assert_eq!(response.status, 200);
        let payload: serde_json::Value =
            serde_json::from_slice(&response.body).expect("metrics json body");
        assert_eq!(payload["connections_active"], 0);
        assert_eq!(payload["bytes_in_total"], 0);
        assert_eq!(payload["bytes_out_total"], 0);
        assert_eq!(payload["driver_errors_total"], 0);
    }

    #[test]
    fn ingress_track_update_replaces_existing_track_metadata() {
        let mut tracks = vec![TrackInfo::new(
            cheetah_codec::TrackId(1),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        )];
        let mut updated = TrackInfo::new(
            cheetah_codec::TrackId(1),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        );
        updated.extradata = cheetah_codec::CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x64])],
            pps: vec![Bytes::from_static(&[0x68, 0xeb])],
            avcc: None,
        };
        updated.refresh_readiness();

        assert!(merge_track_update(&mut tracks, updated.clone()));
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0], updated);
        assert!(tracks[0].is_ready());
    }

    #[test]
    fn ingress_track_update_keeps_distinct_tracks_with_same_codec() {
        let mut tracks = vec![TrackInfo::new(
            cheetah_codec::TrackId(1),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        )];
        let second = TrackInfo::new(
            cheetah_codec::TrackId(2),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        );

        assert!(merge_track_update(&mut tracks, second.clone()));
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[1], second);
    }

    #[test]
    fn egress_wait_requires_non_empty_ready_tracks() {
        let empty = Vec::new();
        assert!(!tracks_ready_for_egress(&empty));

        let pending = vec![TrackInfo::new(
            cheetah_codec::TrackId(1),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        )];
        assert!(!tracks_ready_for_egress(&pending));

        let mut ready = TrackInfo::new(
            cheetah_codec::TrackId(2),
            cheetah_codec::MediaKind::Audio,
            cheetah_codec::CodecId::AAC,
            48_000,
        );
        ready.extradata = cheetah_codec::CodecExtradata::AAC {
            asc: Bytes::from_static(&[0x11, 0x88]),
        };
        ready.refresh_readiness();
        assert!(tracks_ready_for_egress(&[ready]));
    }

    #[test]
    fn egress_wait_accepts_extended_passthrough_codecs() {
        let mut h266 = TrackInfo::new(
            cheetah_codec::TrackId(1),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H266,
            90_000,
        );
        h266.extradata = cheetah_codec::CodecExtradata::H266 {
            vps: vec![Bytes::from_static(&[0x00, 0x70, 0x01])],
            sps: vec![Bytes::from_static(&[0x00, 0x78, 0x01])],
            pps: vec![Bytes::from_static(&[0x00, 0x80, 0x01])],
        };
        h266.refresh_readiness();

        let mut mjpeg = TrackInfo::new(
            cheetah_codec::TrackId(2),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::MJPEG,
            90_000,
        );
        mjpeg.refresh_readiness();

        let mut adpcm = TrackInfo::new(
            cheetah_codec::TrackId(3),
            cheetah_codec::MediaKind::Audio,
            cheetah_codec::CodecId::ADPCM,
            90_000,
        );
        adpcm.refresh_readiness();

        assert!(tracks_ready_for_egress(&[h266, mjpeg, adpcm]));
    }

    #[test]
    fn relay_job_expands_to_ingress_and_egress_caller_connections() {
        let mut config = SrtModuleConfig::default();
        config.relay_jobs.push(SrtRelayJobConfig {
            name: "relay-a".to_string(),
            enabled: true,
            source_url: "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request"
                .to_string(),
            target_url: "srt://127.0.0.1:9002?mode=caller&streamid=#!::r=live/out,m=publish"
                .to_string(),
            stream_key: "relay/source-a".to_string(),
            retry_backoff_ms: 1_000,
            max_retry_backoff_ms: 30_000,
        });

        let plan = build_job_plan(&config).expect("valid relay job plan");
        assert_eq!(plan.connects.len(), 2);

        let publish_forced = plan
            .forced_modes
            .values()
            .filter(|mode| mode.mode == SrtStreamMode::Publish)
            .count();
        let play_forced = plan
            .forced_modes
            .values()
            .filter(|mode| mode.mode == SrtStreamMode::Play)
            .count();
        assert_eq!(publish_forced, 1);
        assert_eq!(play_forced, 1);
    }

    #[test]
    fn publish_auth_accepts_matching_global_token() {
        let mut config = SrtModuleConfig::default();
        config.auth.enabled = true;
        config.auth.publish_token = "publish-secret".to_string();

        let classify = classify_stream(
            &config,
            Some("#!::r=live/test,m=publish,token=publish-secret"),
            None,
            None,
        )
        .expect("matching publish token should pass");

        assert_eq!(classify.mode, SrtStreamMode::Publish);
        assert_eq!(classify.stream_key.to_string(), "live/test");
    }

    #[test]
    fn publish_auth_rejects_missing_or_wrong_token() {
        let mut config = SrtModuleConfig::default();
        config.auth.enabled = true;
        config.auth.publish_token = "publish-secret".to_string();

        let missing = classify_stream(&config, Some("#!::r=live/test,m=publish"), None, None)
            .expect_err("missing publish token should fail");
        assert_eq!(missing, "reject:auth_rejected");

        let wrong = classify_stream(&config, Some("#!::r=live/test,m=publish,token=wrong"), None, None)
            .expect_err("wrong publish token should fail");
        assert_eq!(wrong, "reject:auth_rejected");
    }

    #[test]
    fn request_auth_accepts_matching_user_token() {
        let mut config = SrtModuleConfig::default();
        config.auth.enabled = true;
        config.auth.users.push(SrtAuthUserConfig {
            username: "alice".to_string(),
            token: "alice-secret".to_string(),
        });

        let classify = classify_stream(
            &config,
            Some("#!::r=live/test,m=request,u=alice,token=alice-secret"),
            None,
            None,
        )
        .expect("matching user request token should pass");

        assert_eq!(classify.mode, SrtStreamMode::Request);
        assert_eq!(classify.stream_key.to_string(), "live/test");
    }

    #[test]
    fn caller_job_url_token_is_added_to_stream_id() {
        let config = SrtModuleConfig::default();
        let (_remote, stream_id, _options) = caller_connect_parts(
            "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request&token=query-secret",
            SrtStreamMode::Request,
            "live/local".to_string(),
            &config,
        )
        .expect("valid caller parts");

        assert_eq!(
            stream_id.as_deref(),
            Some("#!::r=live/in,m=request,token=query-secret")
        );
    }

    #[test]
    fn caller_job_stream_id_token_wins_over_url_token() {
        let config = SrtModuleConfig::default();
        let (_remote, stream_id, _options) = caller_connect_parts(
            "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request,token=stream-secret&token=query-secret",
            SrtStreamMode::Request,
            "live/local".to_string(),
            &config,
        )
        .expect("valid caller parts");

        assert_eq!(
            stream_id.as_deref(),
            Some("#!::r=live/in,m=request,token=stream-secret")
        );
    }

    #[test]
    fn caller_job_rejects_invalid_access_control_stream_id() {
        let config = SrtModuleConfig::default();
        let err = caller_connect_parts(
            "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=../secret,m=request&token=query-secret",
            SrtStreamMode::Request,
            "live/local".to_string(),
            &config,
        )
        .expect_err("invalid access-control stream id should fail");

        assert!(err.to_string().contains("invalid stream id"));
        assert!(err.to_string().contains(".."));
    }

    #[test]
    fn caller_job_url_token_is_encoded_for_stream_id_field() {
        let config = SrtModuleConfig::default();
        let (_remote, stream_id, _options) = caller_connect_parts(
            "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request&token=a%2Cb%3Dc%25",
            SrtStreamMode::Request,
            "live/local".to_string(),
            &config,
        )
        .expect("valid caller parts");
        let stream_id = stream_id.expect("merged stream id");
        let parsed = parse_srt_stream_id(&stream_id).expect("merged stream id should parse");

        assert_eq!(
            parsed.auth_params.get("token").map(String::as_str),
            Some("a,b=c%")
        );
    }

    #[test]
    fn caller_job_rejects_empty_url_passphrase() {
        let config = SrtModuleConfig::default();
        let err = caller_connect_parts(
            "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request&passphrase=",
            SrtStreamMode::Request,
            "live/local".to_string(),
            &config,
        )
        .expect_err("empty URL passphrase should fail");

        assert!(err.to_string().contains("passphrase must not be empty"));
    }

    #[test]
    fn relay_job_plan_keeps_retry_metadata_for_each_caller() {
        let mut config = SrtModuleConfig::default();
        config.relay_jobs.push(SrtRelayJobConfig {
            name: "relay-retry".to_string(),
            enabled: true,
            source_url: "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request"
                .to_string(),
            target_url: "srt://127.0.0.1:9002?mode=caller&streamid=#!::r=live/out,m=publish"
                .to_string(),
            stream_key: "relay/retry".to_string(),
            retry_backoff_ms: 250,
            max_retry_backoff_ms: 1_000,
        });

        let plan = build_job_plan(&config).expect("valid relay job plan");

        assert_eq!(plan.jobs.len(), 2);
        assert!(plan
            .jobs
            .values()
            .all(|job| job.retry_backoff_ms == 250 && job.max_retry_backoff_ms == 1_000));
    }

    #[test]
    fn retry_backoff_uses_exponential_cap() {
        assert_eq!(retry_delay_ms(250, 1_000, 0), 250);
        assert_eq!(retry_delay_ms(250, 1_000, 1), 500);
        assert_eq!(retry_delay_ms(250, 1_000, 2), 1_000);
        assert_eq!(retry_delay_ms(250, 1_000, 3), 1_000);
    }

    #[test]
    fn default_no_m_is_play_or_request() {
        let config = SrtModuleConfig::default();
        let classify = classify_stream(&config, Some("#!::r=live/test"), None, None)
            .expect("valid stream id should classify");
        assert!(
            matches!(classify.mode, SrtStreamMode::Request | SrtStreamMode::Play),
            "missing `m` should default to request/play, not publish"
        );
    }

    #[test]
    fn m_publish_is_publish() {
        let config = SrtModuleConfig::default();
        let classify = classify_stream(&config, Some("#!::r=live/test,m=publish"), None, None)
            .expect("publish stream id should classify");
        assert_eq!(classify.mode, SrtStreamMode::Publish);
    }

    #[test]
    fn auth_params_include_m() {
        let config = SrtModuleConfig::default();
        let classify =
            classify_stream(&config, Some("#!::r=live/test,m=publish"), None, None)
                .expect("valid stream id should classify");
        assert_eq!(classify.auth.auth_params.get("m"), Some(&"publish".to_string()));
    }

    #[test]
    fn strict_r_one_segment_fails() {
        let config = SrtModuleConfig::default();
        let err = classify_stream(&config, Some("#!::r=live"), None, None)
            .expect_err("single-segment r should fail");
        assert!(err.contains("reject:invalid_stream_id"));
    }

    #[test]
    fn payload_kind_rejects_non_ts() {
        let config = SrtModuleConfig {
            payload: SrtPayloadModuleConfig {
                kind: "flv".to_string(),
            },
            ..Default::default()
        };
        let err = config.validate().expect_err("non-mpegts payload should fail");
        assert!(err.contains("mpegts"));
    }

    #[test]
    fn fec_required_requires_enabled() {
        let fec = SrtFecModuleConfig {
            enabled: false,
            required: true,
            ..Default::default()
        };
        let err = fec.validate().expect_err("required without enabled should fail");
        assert!(err.contains("enabled"));
    }

    #[test]
    fn fec_enabled_validates_matrix_size() {
        let fec = SrtFecModuleConfig {
            enabled: true,
            cols: 0,
            rows: 5,
            ..Default::default()
        };
        let err = fec.validate().expect_err("zero cols should fail");
        assert!(err.contains("cols"));

        let fec = SrtFecModuleConfig {
            enabled: true,
            cols: 200,
            rows: 200,
            ..Default::default()
        };
        let err = fec.validate().expect_err("oversized matrix should fail");
        assert!(err.contains("matrix"));
    }

    #[test]
    fn fec_config_default_validates() {
        let config = SrtModuleConfig::default();
        config.validate().expect("default config should validate");
    }
}
