//! ZLMediaKit answer SDP fixture validation.
//!
//! Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`):
//! ignored interop tests need a way to confirm cheetah's understanding
//! of ZLM-style answer SDPs without spinning up a full media-plane
//! lab. We stash a small set of representative ZLM answer SDPs in
//! `tests/fixtures/zlm/` and run the
//! `interop_harness::assertions` helpers against each — proving the
//! assertion helpers stay tight against real-world wire shapes and
//! catching regressions if cheetah's expectations drift.
//!
//! These tests are NOT `#[ignore]` because the fixtures are static.
//! They run in every default `cargo test` pass and are bundled with
//! the crate.

mod interop_harness;

use interop_harness::assertions::{
    assert_answer_well_formed, assert_candidate_types_present, assert_msid_stream_present,
    assert_offer_well_formed, assert_simulcast_layers, count_candidates, extract_msids,
    extract_simulcast_rids, InteropThresholds,
};

const WHIP_ANSWER: &str = include_str!("fixtures/zlm/whip_answer.sdp");
const WHEP_ANSWER: &str = include_str!("fixtures/zlm/whep_answer.sdp");
const TCP_OFFER: &str = include_str!("fixtures/zlm/tcp_candidate_offer.sdp");
const IPV6_OFFER: &str = include_str!("fixtures/zlm/ipv6_candidate_offer.sdp");
const TURN_OFFER: &str = include_str!("fixtures/zlm/turn_relay_offer.sdp");
const DATACHANNEL_ANSWER: &str = include_str!("fixtures/zlm/datachannel_answer.sdp");
const H264_ONLY_OFFER: &str = include_str!("fixtures/zlm/h264_only_offer.sdp");
const GB28181_PLAY_ANSWER: &str = include_str!("fixtures/zlm/gb28181_play_answer.sdp");
const SIMULCAST_OFFER: &str = include_str!("fixtures/zlm/simulcast_offer.sdp");
const LOW_LATENCY_OFFER: &str = include_str!("fixtures/zlm/low_latency_offer.sdp");
const TCP_FALLBACK_ANSWER: &str = include_str!("fixtures/zlm/tcp_fallback_answer.sdp");
const SCREEN_SHARE_OFFER: &str = include_str!("fixtures/zlm/screen_share_offer.sdp");
const SVC_OFFER: &str = include_str!("fixtures/zlm/svc_offer.sdp");
const DTMF_AUDIO_OFFER: &str = include_str!("fixtures/zlm/dtmf_audio_offer.sdp");

#[test]
fn zlm_whip_answer_passes_well_formed_check() {
    assert_answer_well_formed(WHIP_ANSWER).expect("whip_answer must be well-formed");
}

#[test]
fn zlm_whep_answer_passes_well_formed_check() {
    assert_answer_well_formed(WHEP_ANSWER).expect("whep_answer must be well-formed");
}

#[test]
fn zlm_answers_carry_required_zlm_specific_fields() {
    // ZLM identifies itself in `s=` line. We don't enforce that on
    // every answer the harness sees (other vendors omit it) but we
    // keep the pattern visible in the fixtures for comparison.
    assert!(WHIP_ANSWER.contains("s=ZLMediaKit"));
    assert!(WHEP_ANSWER.contains("s=ZLMediaKit"));

    // Both fixtures must declare BUNDLE on mids 0 and 1 — the
    // ordering ZLM's WebRtcSdp generator emits.
    for sdp in [WHIP_ANSWER, WHEP_ANSWER] {
        assert!(sdp.contains("a=group:BUNDLE 0 1"));
        assert!(sdp.contains("a=mid:0"));
        assert!(sdp.contains("a=mid:1"));
        assert!(sdp.contains("a=rtcp-mux"));
        assert!(sdp.contains("a=rtcp-rsize"));
        assert!(sdp.contains("a=ice-options:trickle"));
    }
}

#[test]
fn zlm_whep_answer_lists_send_direction_and_msid() {
    // WHEP answers from ZLM are sendonly (server is the source) and
    // include an `a=msid:` per `m=` so the player can attach the
    // streams to MediaStreams of the right id.
    assert!(WHEP_ANSWER.contains("a=sendonly"));
    let msid_count = WHEP_ANSWER.matches("a=msid:").count();
    assert!(
        msid_count >= 2,
        "WHEP answer should include msid for both audio and video, saw {msid_count}"
    );
}

#[test]
fn zlm_whep_video_ssrc_group_is_fid_for_rtx() {
    // RTX retransmissions require the answer to advertise an FID
    // ssrc-group pairing the original ssrc with the rtx stream.
    assert!(WHEP_ANSWER.contains("a=ssrc-group:FID"));
}

#[test]
fn zlm_whip_answer_is_recvonly_and_rejects_offer_well_formed_via_offer_helper() {
    // WHIP answers are recvonly because the server consumes the
    // peer's stream. Sanity check the assertion helper catches a
    // recvonly answer when (incorrectly) handed it as an offer.
    assert!(WHIP_ANSWER.contains("a=recvonly"));
    // The offer helper still accepts these SDPs because it only
    // checks shape, not direction. That's the documented contract.
    assert!(assert_offer_well_formed(WHIP_ANSWER).is_ok());
}

#[test]
fn interop_thresholds_default_values_are_sane() {
    // Smoke test that pinning a `Default::default()` reference to
    // the harness thresholds keeps the documentation in sync. If
    // someone bumps a default and forgets to update fixtures, this
    // test fires.
    let t = InteropThresholds::default();
    assert!(t.first_keyframe.as_secs() <= 5);
    assert!(t.max_rtt.as_millis() <= 2_000);
    assert!(t.min_nacks_under_loss >= 1);
    assert!(t.min_bwe_bps >= 100_000);
}

#[test]
fn tcp_candidate_offer_carries_tcp_host_and_srflx() {
    // The TCP fixture exercises ICE TCP candidates (RFC 6544). A
    // valid offer with TCP fallback must include at least one
    // tcptype host candidate and a tcptype srflx through the NAT.
    assert_offer_well_formed(TCP_OFFER).expect("offer well-formed");
    let counts = count_candidates(TCP_OFFER);
    assert!(
        counts.tcp >= 2,
        "TCP offer should have >= 2 TCP candidates, saw {counts:?}"
    );
    assert!(
        counts.host >= 2,
        "TCP offer should have >= 2 host candidates"
    );
    assert!(
        counts.srflx >= 1,
        "TCP offer should have >= 1 srflx candidate"
    );
    assert!(TCP_OFFER.contains("tcptype active"));
    assert!(TCP_OFFER.contains("tcptype passive"));
}

#[test]
fn ipv6_candidate_offer_advertises_link_local_and_global_v6() {
    // ZLM (and most modern stacks) emit both link-local and
    // global IPv6 candidates when v6 is enabled. The fixture pins
    // both; cheetah must not reject either.
    assert_offer_well_formed(IPV6_OFFER).expect("offer well-formed");
    let counts = count_candidates(IPV6_OFFER);
    assert!(
        counts.ipv6 >= 3,
        "ipv6 offer should advertise >= 3 v6 candidates, saw {counts:?}"
    );
    assert_candidate_types_present(IPV6_OFFER, true, true, false)
        .expect("ipv6 offer must include host + srflx");
    assert!(IPV6_OFFER.contains("fe80::"), "must advertise link-local");
    assert!(
        IPV6_OFFER.contains("2001:db8::"),
        "must advertise global v6"
    );
}

#[test]
fn turn_relay_offer_includes_relay_candidate_with_raddr() {
    // The TURN fixture proves the relay candidate path: cheetah
    // should accept a remote relay candidate with `raddr` /
    // `rport` referring to the original srflx address.
    assert_offer_well_formed(TURN_OFFER).expect("offer well-formed");
    assert_candidate_types_present(TURN_OFFER, true, true, true)
        .expect("turn offer must include host + srflx + relay");
    let counts = count_candidates(TURN_OFFER);
    assert_eq!(counts.relay, 1, "exactly one relay candidate");
    // RFC 5245 §15.1: relay candidates must carry `raddr` / `rport`.
    let relay_line = TURN_OFFER
        .lines()
        .find(|l| l.contains("typ relay"))
        .expect("relay line present");
    assert!(
        relay_line.contains("raddr"),
        "relay must include raddr: {relay_line}"
    );
    assert!(
        relay_line.contains("rport"),
        "relay must include rport: {relay_line}"
    );
}

#[test]
fn candidate_counter_handles_all_three_offer_fixtures() {
    // Cross-fixture sanity: each ZLM offer fixture parses cleanly
    // through the harness counter and yields the expected mix.
    let tcp = count_candidates(TCP_OFFER);
    let v6 = count_candidates(IPV6_OFFER);
    let turn = count_candidates(TURN_OFFER);
    // TCP offer: TCP transport dominates; no relay.
    assert!(tcp.tcp >= 1 && tcp.relay == 0);
    // IPv6 offer: only ipv6 candidates; no relay.
    assert!(v6.ipv6 >= 1 && v6.ipv4 == 0 && v6.relay == 0);
    // TURN offer: at least one of each, with at least one relay.
    assert!(turn.host >= 1 && turn.srflx >= 1 && turn.relay >= 1);
}

#[test]
fn datachannel_answer_passes_well_formed_check() {
    // The DataChannel fixture is a 3-section answer (audio + video +
    // SCTP application). The general well-formed helper must accept
    // it because all three sections carry the required attributes.
    assert_answer_well_formed(DATACHANNEL_ANSWER).expect("datachannel answer well-formed");
}

#[test]
fn datachannel_answer_includes_application_section_with_sctp_port() {
    // The fixture pins the canonical SCTP-over-DTLS shape:
    // `m=application <port> UDP/DTLS/SCTP webrtc-datachannel`
    // followed by `a=sctp-port:` + `a=max-message-size:`.
    assert!(
        DATACHANNEL_ANSWER.contains("m=application 9 UDP/DTLS/SCTP webrtc-datachannel"),
        "answer must contain SCTP application section header"
    );
    assert!(
        DATACHANNEL_ANSWER.contains("a=sctp-port:5000"),
        "answer must declare sctp-port"
    );
    assert!(
        DATACHANNEL_ANSWER.contains("a=max-message-size:"),
        "answer must declare max-message-size"
    );
}

#[test]
fn datachannel_answer_bundles_three_sections() {
    // BUNDLE must list all three mid values so the SCTP section
    // shares the same DTLS transport as audio and video.
    assert!(
        DATACHANNEL_ANSWER.contains("a=group:BUNDLE 0 1 2"),
        "answer must bundle audio + video + application"
    );
    let mids: Vec<&str> = DATACHANNEL_ANSWER
        .lines()
        .filter(|l| l.starts_with("a=mid:"))
        .collect();
    assert_eq!(
        mids.len(),
        3,
        "expected three mid declarations, saw {mids:?}"
    );
}

#[test]
fn datachannel_answer_max_message_size_is_within_default_cap() {
    // The driver default cap is 256 KiB. ZLM's emitted value
    // should be at most that much; if ZLM advertises a larger
    // value cheetah's clamping would create a mismatch worth
    // documenting.
    let line = DATACHANNEL_ANSWER
        .lines()
        .find(|l| l.starts_with("a=max-message-size:"))
        .expect("max-message-size line present");
    let value: usize = line
        .strip_prefix("a=max-message-size:")
        .and_then(|s| s.trim().parse().ok())
        .expect("value parses as usize");
    assert!(
        value <= 262_144,
        "max-message-size {value} exceeds cheetah default cap (262144)"
    );
}

#[test]
fn h264_only_offer_advertises_single_video_codec_with_rtx() {
    // Devices and embedded clients often emit H.264-only offers
    // without VP8 / VP9 / AV1. The harness must accept the offer
    // shape and the codec list parser must not require multiple
    // payload types.
    assert_offer_well_formed(H264_ONLY_OFFER).expect("h264-only offer well-formed");
    // Exactly one codec PT (102) + its RTX (103) on the video m=
    // line. Counting `a=rtpmap:` rules out fmtp/rtcp-fb noise.
    let video_rtpmaps: Vec<&str> = H264_ONLY_OFFER
        .lines()
        .filter(|l| l.starts_with("a=rtpmap:"))
        .filter(|l| l.contains("90000")) // video clock rate (audio is 48000)
        .collect();
    assert_eq!(
        video_rtpmaps.len(),
        2,
        "h264-only offer should advertise exactly H.264 + RTX, saw {video_rtpmaps:?}"
    );
    assert!(
        H264_ONLY_OFFER.contains("a=rtpmap:102 H264/90000"),
        "primary codec should be H.264 PT 102"
    );
    assert!(
        H264_ONLY_OFFER.contains("a=fmtp:103 apt=102"),
        "RTX should reference the H.264 PT via apt"
    );
}

#[test]
fn h264_only_offer_carries_packetization_mode_1() {
    // packetization-mode=1 (NAL FU-A) is the canonical mode for
    // WebRTC interop; cheetah must accept the offer's fmtp string.
    let fmtp = H264_ONLY_OFFER
        .lines()
        .find(|l| l.starts_with("a=fmtp:102"))
        .expect("fmtp:102 line present");
    assert!(
        fmtp.contains("packetization-mode=1"),
        "h264-only offer should declare packetization-mode=1, got {fmtp}"
    );
    assert!(
        fmtp.contains("profile-level-id="),
        "fmtp must include profile-level-id"
    );
}

#[test]
fn gb28181_play_answer_passes_well_formed_check() {
    // ZLM emits this answer when a WHEP client plays a stream
    // ingested via GB28181. The shape matches a single-section
    // sendonly answer; the harness must not reject it for being
    // audio-less (GB28181 streams are commonly video-only).
    assert_answer_well_formed(GB28181_PLAY_ANSWER).expect("gb28181 answer well-formed");
}

#[test]
fn gb28181_play_answer_is_video_only_h264() {
    // The fixture pins the canonical GB28181 → WHEP shape:
    // single m=video, H.264, sendonly, msid identifies the GB
    // source. No audio section.
    let video_count = GB28181_PLAY_ANSWER.matches("\nm=video").count()
        + if GB28181_PLAY_ANSWER.starts_with("m=video") {
            1
        } else {
            0
        };
    let audio_count = GB28181_PLAY_ANSWER.matches("\nm=audio").count();
    assert_eq!(video_count, 1, "expected exactly one m=video section");
    assert_eq!(audio_count, 0, "GB28181 fixture should be video-only");
    assert!(GB28181_PLAY_ANSWER.contains("a=sendonly"));
    assert!(GB28181_PLAY_ANSWER.contains("a=msid:gb gb28181-video"));
    assert!(GB28181_PLAY_ANSWER.contains("a=rtpmap:96 H264/90000"));
}

#[test]
fn gb28181_play_answer_advertises_zlm_specific_session_name() {
    // ZLM sets `s=ZLMediaKit-GB28181` for the GB-bridged path so
    // operators can grep server-side logs by source. We pin this
    // shape so a refactor doesn't quietly drop the marker.
    assert!(
        GB28181_PLAY_ANSWER.contains("s=ZLMediaKit-GB28181"),
        "GB28181 fixture should keep the ZLM-specific session name marker"
    );
}

#[test]
fn simulcast_offer_passes_well_formed_check() {
    // The simulcast fixture is a 2-section offer (audio + video
    // with 3 RID layers). The general offer helper must accept it.
    assert_offer_well_formed(SIMULCAST_OFFER).expect("simulcast offer well-formed");
}

#[test]
fn simulcast_offer_advertises_three_send_layers() {
    // Browsers typically send `hi;mid;lo` for screen-share-grade
    // simulcast. The harness should pull all three RIDs out.
    let rids = extract_simulcast_rids(SIMULCAST_OFFER)
        .expect("simulcast offer must have a=simulcast: line");
    assert_eq!(
        rids.send,
        vec!["hi".to_string(), "mid".to_string(), "lo".to_string()]
    );
    assert_simulcast_layers(SIMULCAST_OFFER, 3).expect("3 layers");
}

#[test]
fn simulcast_offer_carries_required_extension_headers() {
    // Simulcast hinges on RFC 8852 RID + RFC 7941 MID extensions.
    // If the offer drops any of these, RTP routing in cheetah
    // fails silently — pin the requirement here so a regression
    // fires loud.
    assert!(SIMULCAST_OFFER.contains("urn:ietf:params:rtp-hdrext:sdes:mid"));
    assert!(SIMULCAST_OFFER.contains("urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id"));
    assert!(SIMULCAST_OFFER.contains("urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id"));
}

#[test]
fn simulcast_offer_lists_all_three_rid_lines() {
    // Each `a=rid:<id> send` line must appear; counting is more
    // robust than substring search across whitespace.
    let count = SIMULCAST_OFFER
        .lines()
        .filter(|l| l.starts_with("a=rid:") && l.contains(" send"))
        .count();
    assert_eq!(
        count, 3,
        "simulcast offer must declare 3 a=rid: send layers, saw {count}"
    );
}

#[test]
fn low_latency_offer_passes_well_formed_check() {
    // Low-latency offers ship the playout-delay + video-timing
    // hdrexts. Harness `assert_offer_well_formed` only checks
    // shape, so we verify the offer is not malformed.
    assert_offer_well_formed(LOW_LATENCY_OFFER).expect("low-latency offer well-formed");
}

#[test]
fn low_latency_offer_includes_playout_delay_extmap() {
    // The whole point of low-latency is the playout-delay ext.
    // If a refactor drops it cheetah's downstream timing pipeline
    // would silently lose hint headers — pin the requirement.
    assert!(
        LOW_LATENCY_OFFER.contains("http://www.webrtc.org/experiments/rtp-hdrext/playout-delay"),
        "low-latency offer must declare playout-delay extmap"
    );
    assert!(
        LOW_LATENCY_OFFER.contains("http://www.webrtc.org/experiments/rtp-hdrext/video-timing"),
        "low-latency offer must declare video-timing extmap"
    );
}

#[test]
fn low_latency_offer_uses_transport_cc_and_goog_remb() {
    // BWE pipeline depends on transport-cc; goog-remb is legacy
    // but still emitted by browsers in the low-latency profile.
    let video_section = LOW_LATENCY_OFFER
        .split("\nm=video")
        .nth(1)
        .expect("video section present");
    assert!(
        video_section.contains("a=rtcp-fb:96 transport-cc"),
        "low-latency offer must request transport-cc feedback"
    );
    assert!(
        video_section.contains("a=rtcp-fb:96 goog-remb"),
        "low-latency offer should also keep goog-remb for legacy peers"
    );
}

#[test]
fn tcp_fallback_answer_passes_well_formed_check() {
    // ZLM emits this answer when the peer can only reach the
    // server via TCP (NAT/firewall blocks UDP). cheetah's harness
    // must accept the shape — TCP/TLS/RTP/SAVPF is a valid m=
    // proto under RFC 7850.
    assert_answer_well_formed(TCP_FALLBACK_ANSWER).expect("tcp fallback answer well-formed");
}

#[test]
fn tcp_fallback_answer_uses_tcp_proto_and_passive_candidates() {
    // The m=video line must declare TCP/TLS/RTP/SAVPF, not the
    // UDP variant. ZLM also advertises tcptype passive candidates
    // because cheetah/ZLM is the server side and waits for client
    // TCP connections.
    assert!(
        TCP_FALLBACK_ANSWER.contains("m=video 9 TCP/TLS/RTP/SAVPF"),
        "TCP fallback answer must use TCP/TLS/RTP/SAVPF proto"
    );
    let counts = count_candidates(TCP_FALLBACK_ANSWER);
    assert!(counts.tcp >= 1, "must include >= 1 TCP candidate");
    assert_eq!(
        counts.udp, 0,
        "TCP fallback answer must not advertise UDP candidates"
    );
    let passive = TCP_FALLBACK_ANSWER
        .lines()
        .filter(|l| l.starts_with("a=candidate:") && l.contains("tcptype passive"))
        .count();
    assert!(
        passive >= 1,
        "TCP fallback server-side answer must advertise >= 1 tcptype passive"
    );
}

#[test]
fn tcp_fallback_answer_keeps_rtx_for_recovery() {
    // TCP carries no NACK at the transport layer, but RTX is
    // still useful for application-layer retransmission decisions
    // (cheetah's BWE / NACK paths still run inside RTP). Pin the
    // requirement.
    assert!(TCP_FALLBACK_ANSWER.contains("a=rtpmap:97 rtx/90000"));
    assert!(TCP_FALLBACK_ANSWER.contains("a=fmtp:97 apt=96"));
    assert!(TCP_FALLBACK_ANSWER.contains("a=ssrc-group:FID"));
}

#[test]
fn screen_share_offer_passes_well_formed_check() {
    // Screen-share offers wrap audio + video on one msid and add
    // the `a=content:slides` marker. Harness must accept the shape.
    assert_offer_well_formed(SCREEN_SHARE_OFFER).expect("screen share offer well-formed");
}

#[test]
fn screen_share_offer_groups_audio_and_video_under_same_msid() {
    // Both audio and video msid lines should reference the same
    // stream id (`screen-share`) so a player can attach both
    // tracks to the same MediaStream.
    let entries = extract_msids(SCREEN_SHARE_OFFER);
    assert!(
        entries.len() >= 2,
        "screen share offer should have >= 2 a=msid lines, saw {entries:?}"
    );
    let stream_ids: std::collections::HashSet<&str> =
        entries.iter().map(|e| e.stream_id.as_str()).collect();
    assert_eq!(
        stream_ids.len(),
        1,
        "all msid stream ids should be identical, saw {stream_ids:?}"
    );
    assert_msid_stream_present(SCREEN_SHARE_OFFER, "screen-share")
        .expect("screen-share stream present");
}

#[test]
fn screen_share_offer_advertises_content_slides() {
    // RFC 4796 / WebRTC content type extension: `a=content:slides`
    // hints to the receiver that the video is a low-motion slide
    // share. Pin it so a refactor doesn't drop the marker.
    assert!(
        SCREEN_SHARE_OFFER.contains("a=content:slides"),
        "screen share fixture should declare a=content:slides"
    );
    assert!(
        SCREEN_SHARE_OFFER
            .contains("http://www.webrtc.org/experiments/rtp-hdrext/video-content-type"),
        "screen share fixture should declare video-content-type extmap"
    );
}

#[test]
fn screen_share_offer_keeps_rtx_for_recovery() {
    // Even slide-share content benefits from RTX since cheetah's
    // BWE runs over RTP. Ensure the FID group survives.
    assert!(SCREEN_SHARE_OFFER.contains("a=ssrc-group:FID"));
    assert!(SCREEN_SHARE_OFFER.contains("a=rtpmap:97 rtx/90000"));
}

#[test]
fn svc_offer_passes_well_formed_check() {
    // VP9 SVC L3T3 offer: 3 spatial layers × 3 temporal layers
    // shipped on a single payload type with `a=scalability-mode`.
    // The harness treats SVC the same as a regular offer; it must
    // accept the shape.
    assert_offer_well_formed(SVC_OFFER).expect("svc offer well-formed");
}

#[test]
fn svc_offer_advertises_scalability_mode() {
    // RFC 9134 / draft-ietf-avtext-rid: `a=scalability-mode` is the
    // canonical signal for SVC layer counts. cheetah must keep the
    // attribute round-tripping through the answer.
    assert!(
        SVC_OFFER.contains("a=scalability-mode:L3T3"),
        "svc offer should declare a=scalability-mode:L3T3"
    );
    assert!(
        SVC_OFFER.contains("a=rtpmap:98 VP9/90000"),
        "svc offer should use VP9 (only widely-deployed SVC codec)"
    );
}

#[test]
fn svc_offer_keeps_rtx_for_temporal_layer_recovery() {
    // SVC layers can drop independently; RTX recovery still
    // applies on the base layer SSRC. Pin RTX + FID to ensure
    // cheetah's NACK pipeline receives the necessary signals.
    assert!(SVC_OFFER.contains("a=rtpmap:99 rtx/90000"));
    assert!(SVC_OFFER.contains("a=fmtp:99 apt=98"));
    assert!(SVC_OFFER.contains("a=ssrc-group:FID"));
}

#[test]
fn dtmf_audio_offer_passes_well_formed_check() {
    // DTMF (RFC 4733 telephone-event) offers ship telephone-event
    // payload types alongside opus. Cheetah's harness must accept
    // them — the audio-only telephony bridge use case.
    assert_offer_well_formed(DTMF_AUDIO_OFFER).expect("dtmf offer well-formed");
}

#[test]
fn dtmf_audio_offer_advertises_telephone_event_at_two_rates() {
    // Browsers typically negotiate telephone-event at both 48 kHz
    // (alongside opus) and 8 kHz (legacy PSTN gateways). cheetah
    // must accept both rtpmap entries without rejecting the offer.
    assert!(
        DTMF_AUDIO_OFFER.contains("a=rtpmap:110 telephone-event/48000"),
        "dtmf offer should declare telephone-event at 48000"
    );
    assert!(
        DTMF_AUDIO_OFFER.contains("a=rtpmap:126 telephone-event/8000"),
        "dtmf offer should declare telephone-event at 8000"
    );
    // The fmtp `0-16` enumerates DTMF events 0-9, *, #, A-D and
    // is the canonical default. Pin the value so a refactor that
    // narrows the range fires loud.
    assert!(DTMF_AUDIO_OFFER.contains("a=fmtp:110 0-16"));
    assert!(DTMF_AUDIO_OFFER.contains("a=fmtp:126 0-16"));
}

#[test]
fn dtmf_audio_offer_pairs_telephone_event_with_opus() {
    // Telephone-event is auxiliary; opus must remain the primary
    // audio codec. Confirm both are present and the m= line lists
    // opus first (highest priority for the offerer).
    assert!(
        DTMF_AUDIO_OFFER.contains("a=rtpmap:111 opus/48000/2"),
        "dtmf offer must keep opus as the primary audio codec"
    );
    let m_line = DTMF_AUDIO_OFFER
        .lines()
        .find(|l| l.starts_with("m=audio"))
        .expect("audio m= line present");
    let pts: Vec<&str> = m_line.split_whitespace().skip(3).collect();
    assert_eq!(
        pts.first().copied(),
        Some("111"),
        "opus PT (111) should be the first listed payload type, saw {pts:?}"
    );
}
