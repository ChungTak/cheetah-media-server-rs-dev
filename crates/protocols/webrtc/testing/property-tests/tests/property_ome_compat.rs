//! Property tests for OME URL / signaling compatibility helpers.
//!
//! OME URL / 信令兼容性 helper 的属性测试。

use cheetah_webrtc_module::compat::{
    parse_ome_transport_mode, parse_ome_webrtc_path_query_with_default_transport, OmeTransportMode,
};
use cheetah_webrtc_module::ome_signaling::{parse_ome_ws_message, OmeWsDecoderConfig};
use proptest::prelude::*;

/// Generate a valid URL path segment.
///
/// 生成有效 URL 路径段。
fn arb_seg() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_\\-]{1,24}"
}

proptest! {
    /// OME URL parsing is total for generated path/query inputs.
    ///
    /// 生成路径/查询输入下 OME URL 解析全覆盖。
    #[test]
    fn ome_url_parse_is_total_for_generated_inputs(
        app in arb_seg(),
        stream in arb_seg(),
        playlist in proptest::option::of(arb_seg()),
        query in any::<String>(),
    ) {
        let path = match playlist {
            Some(pl) => format!("/{app}/{stream}/{pl}"),
            None => format!("/{app}/{stream}"),
        };
        let _ = parse_ome_webrtc_path_query_with_default_transport(
            &path,
            Some(&query),
            OmeTransportMode::UdpTcp,
        );
    }

    /// Known OME transport literals round-trip through a canonical form.
    ///
    /// 已知 OME 传输字面量通过规范形式往返。
    #[test]
    fn ome_transport_known_literals_roundtrip(
        input in prop_oneof![
            Just("udp".to_string()),
            Just("tcp".to_string()),
            Just("relay".to_string()),
            Just("udptcp".to_string()),
            Just("all".to_string()),
            Just("UDP-TCP".to_string()),
            Just("TURN".to_string()),
        ]
    ) {
        let parsed = parse_ome_transport_mode(&input).expect("known mode must parse");
        let canonical = match parsed {
            OmeTransportMode::Udp => "udp",
            OmeTransportMode::Tcp => "tcp",
            OmeTransportMode::Relay => "relay",
            OmeTransportMode::UdpTcp => "udptcp",
            OmeTransportMode::All => "all",
        };
        let reparsed = parse_ome_transport_mode(canonical).expect("canonical mode must parse");
        prop_assert_eq!(parsed, reparsed);
    }

    /// OME WebSocket decoder never panics on arbitrary JSON.
    ///
    /// OME WebSocket 解码器对任意 JSON 不 panic。
    #[test]
    fn ome_ws_decoder_never_panics_on_arbitrary_json(input in any::<String>()) {
        let _ = parse_ome_ws_message(&input, OmeWsDecoderConfig::default());
    }
}
