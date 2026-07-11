//! ZLM-compatible interoperability tests for SRT.
//!
//! These tests are ignored by default and only run when `CHEETAH_SRT_INTEROP=1` is
//! set and a running cheetah server is available. They document the matrix used in
//! `dev-docs/plans-28-srt-zlm/interop-results.md`.

#[tokio::test]
#[ignore = "run with CHEETAH_SRT_INTEROP=1 and a running server"]
async fn p1_ffmpeg_publish() {
    if std::env::var("CHEETAH_SRT_INTEROP").as_deref() != Ok("1") {
        return;
    }
    // Example command:
    // ffmpeg -re -stream_loop -1 -i test.ts -c copy -f mpegts \
    //   "srt://127.0.0.1:9000?streamid=#!::r=live/test,m=publish"
    panic!("manual interop test: run ffmpeg publish and verify live/test is ready");
}

#[tokio::test]
#[ignore = "run with CHEETAH_SRT_INTEROP=1 and a running server"]
async fn l1_ffplay_play() {
    if std::env::var("CHEETAH_SRT_INTEROP").as_deref() != Ok("1") {
        return;
    }
    // Example command:
    // ffplay -i "srt://127.0.0.1:9000?streamid=#!::r=live/test"
    panic!("manual interop test: run ffplay play and verify playback starts");
}

#[tokio::test]
#[ignore = "run with CHEETAH_SRT_INTEROP=1 and a running server"]
async fn p2_obs_publish() {
    if std::env::var("CHEETAH_SRT_INTEROP").as_deref() != Ok("1") {
        return;
    }
    // OBS: service=Custom, server=srt://127.0.0.1:9000,
    // stream key / streamid=#!::r=live/test,m=publish
    panic!("manual interop test: publish from OBS and verify live/test is ready");
}

#[tokio::test]
#[ignore = "run with CHEETAH_SRT_INTEROP=1 and a running server"]
async fn l2_vlc_play() {
    if std::env::var("CHEETAH_SRT_INTEROP").as_deref() != Ok("1") {
        return;
    }
    // VLC: open network stream srt://127.0.0.1:9000,
    // set streamid in preferences to #!::r=live/test
    panic!("manual interop test: play with VLC and verify playback starts");
}

#[tokio::test]
#[ignore = "run with CHEETAH_SRT_INTEROP=1 and a running server"]
async fn p3_encrypted_publish_play() {
    if std::env::var("CHEETAH_SRT_INTEROP").as_deref() != Ok("1") {
        return;
    }
    panic!("manual interop test: publish/play with matching passphrase");
}

#[tokio::test]
#[ignore = "run with CHEETAH_SRT_INTEROP=1 and a running server"]
async fn p4_wrong_passphrase_fails() {
    if std::env::var("CHEETAH_SRT_INTEROP").as_deref() != Ok("1") {
        return;
    }
    panic!("manual interop test: mismatched passphrase must fail");
}

#[tokio::test]
#[ignore = "run with CHEETAH_SRT_INTEROP=1 and a running server"]
async fn n1_missing_m_defaults_to_play() {
    if std::env::var("CHEETAH_SRT_INTEROP").as_deref() != Ok("1") {
        return;
    }
    // #!::r=live/test (no m) must not create a publish lease.
    panic!("manual interop test: no m defaults to play; publish lease not created");
}

#[tokio::test]
#[ignore = "run with CHEETAH_SRT_INTEROP=1 and a running server"]
async fn n2_double_publish_same_key_rejects() {
    if std::env::var("CHEETAH_SRT_INTEROP").as_deref() != Ok("1") {
        return;
    }
    panic!("manual interop test: second publisher on same stream key is rejected");
}

#[tokio::test]
#[ignore = "run with CHEETAH_SRT_INTEROP=1 and a running server"]
async fn n3_wrong_token_rejects() {
    if std::env::var("CHEETAH_SRT_INTEROP").as_deref() != Ok("1") {
        return;
    }
    panic!("manual interop test: auth enabled with wrong token is rejected");
}

#[tokio::test]
#[ignore = "run with CHEETAH_SRT_INTEROP=1 and a running server"]
async fn n4_invalid_stream_id_rejects() {
    if std::env::var("CHEETAH_SRT_INTEROP").as_deref() != Ok("1") {
        return;
    }
    // #!::r=live (single segment) must be rejected.
    panic!("manual interop test: invalid stream id is rejected");
}

#[tokio::test]
#[ignore = "run with CHEETAH_SRT_INTEROP=1 and a running server"]
async fn n5_fec_required_without_peer_fec_rejects() {
    if std::env::var("CHEETAH_SRT_INTEROP").as_deref() != Ok("1") {
        return;
    }
    panic!("manual interop test: fec.required=true without peer FEC support rejects");
}
