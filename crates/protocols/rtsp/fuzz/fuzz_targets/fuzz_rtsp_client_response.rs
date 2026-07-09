#![no_main]

use bytes::Bytes;
use cheetah_rtsp_core::{
    CoreInput, RtspCore, RtspMessageLimits, RtspResponseDecoder, RtspRtpInfo, RtspSession,
    RtspTransport,
};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    let bounded = &data[..data.len().min(MAX_INPUT_BYTES)];
    let stream = build_client_response_stream(bounded);
    let limits = RtspMessageLimits::default();

    let mut core = RtspCore::with_limits(limits.clone());
    let mut decoder = RtspResponseDecoder::with_limits(limits);

    let chunk = usize::from(bounded.first().copied().unwrap_or(0) % 32) + 1;
    for piece in stream.chunks(chunk) {
        let _ = core.handle_input(CoreInput::Bytes(Bytes::copy_from_slice(piece)));
        if decoder.feed(piece).is_err() {
            continue;
        }

        loop {
            match decoder.decode() {
                Ok(Some(response)) => {
                    if let Some(session) = response.header_value("Session") {
                        let _ = RtspSession::parse(session);
                    }
                    if let Some(transport) = response.header_value("Transport") {
                        let _ = RtspTransport::parse_multiple(transport);
                    }
                    if let Some(rtp_info) = response.header_value("RTP-Info") {
                        let _ = RtspRtpInfo::parse(rtp_info);
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
    }

    let _ = core.handle_input(CoreInput::PeerClosed);
});

fn build_client_response_stream(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let count = usize::from(data.get(1).copied().unwrap_or(0) % 4) + 1;

    for i in 0..count {
        let status = match data.get(2 + i).copied().unwrap_or(0) % 4 {
            0 => 200,
            1 => 401,
            2 => 454,
            _ => 461,
        };
        let cseq = i + 1;
        let body_len = usize::from(data.get(8 + i).copied().unwrap_or(0) % 32);

        out.extend_from_slice(
            format!(
                "RTSP/1.0 {} STATUS\r\nCSeq: {}\r\nSession: abc{};timeout={}\r\nTransport: RTP/AVP/TCP;unicast;interleaved=0-1\r\nRTP-Info: url=rtsp://example.com/live/trackID=1;seq={};rtptime={}\r\nContent-Length: {}\r\n\r\n",
                status,
                cseq,
                i,
                30 + i,
                1000 + i,
                90000 + i,
                body_len
            )
            .as_bytes(),
        );

        for j in 0..body_len {
            let idx = if data.is_empty() { 0 } else { (10 + i + j) % data.len() };
            out.push(data.get(idx).copied().unwrap_or(0));
        }
    }

    out
}
