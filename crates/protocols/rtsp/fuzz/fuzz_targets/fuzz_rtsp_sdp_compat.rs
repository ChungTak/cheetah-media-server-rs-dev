#![no_main]

use cheetah_rtsp_core::Sdp;
use libfuzzer_sys::fuzz_target;

const MAX_TEXT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    let bounded = &data[..data.len().min(MAX_TEXT_BYTES)];

    if let Ok(text) = std::str::from_utf8(bounded) {
        fuzz_sdp_roundtrip(text);
    }

    let vendor_like = build_vendor_like_sdp(bounded);
    fuzz_sdp_roundtrip(&vendor_like);
});

fn fuzz_sdp_roundtrip(text: &str) {
    if let Ok(sdp) = Sdp::parse(text) {
        let rendered = sdp.to_string();
        let _ = Sdp::parse(&rendered);

        for media in sdp.media.iter().take(16) {
            let _ = media.attributes.len();
            let _ = media.port;
            let _ = media.formats.len();
        }
    }
}

fn build_vendor_like_sdp(data: &[u8]) -> String {
    let payload_video = data.first().copied().unwrap_or(96) % 128;
    let payload_audio = data.get(1).copied().unwrap_or(97) % 128;
    let track_video = data.get(2).copied().unwrap_or(1);
    let track_audio = data.get(3).copied().unwrap_or(2);

    format!(
        "v=0\r\n\
         o=- 1 1 IN IP4 127.0.0.1\r\n\
         s=Cheetah-Compat\r\n\
         t=0 0\r\n\
         a=control:*\r\n\
         m=video 0 RTP/AVP {}\r\n\
         a=rtpmap:{} H264/90000\r\n\
         a=fmtp:{} packetization-mode=1;profile-level-id=42A01E;sprop-parameter-sets=Z0IAH5WoFAFuQA==,aM48gA==\r\n\
         a=control:trackID={}\r\n\
         m=audio 0 RTP/AVP {}\r\n\
         a=rtpmap:{} MPEG4-GENERIC/44100/2\r\n\
         a=fmtp:{} profile-level-id=1;mode=AAC-hbr;config=1210\r\n\
         a=control:trackID={}\r\n",
        payload_video,
        payload_video,
        payload_video,
        track_video,
        payload_audio,
        payload_audio,
        payload_audio,
        track_audio
    )
}
