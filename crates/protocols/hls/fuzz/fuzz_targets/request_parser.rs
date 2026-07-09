#![no_main]
use cheetah_hls_core::{parse_hls_request, HlsRequestKind};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };

    match parse_hls_request(s) {
        Ok(HlsRequestKind::MasterPlaylist { stream_key }) => {
            assert!(!stream_key.namespace.is_empty());
            assert!(!stream_key.stream_path.is_empty());
        }
        Ok(HlsRequestKind::MediaPlaylist { stream_key, .. }) => {
            assert!(!stream_key.namespace.is_empty());
            assert!(!stream_key.stream_path.is_empty());
        }
        Ok(HlsRequestKind::Segment {
            stream_key,
            segment_name,
            ..
        }) => {
            assert!(!stream_key.namespace.is_empty());
            assert!(!stream_key.stream_path.is_empty());
            assert!(!segment_name.is_empty());
        }
        Err(_) => {}
    }
});
