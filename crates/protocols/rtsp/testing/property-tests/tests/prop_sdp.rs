//! Property-based tests for SDP session description parsing.
//!
//! These tests cover parser robustness against arbitrary input, well-formed SDP
//! round-trips, origin (`o=`), connection (`c=`), media (`m=`), and attribute
//! parsing, including `rtpmap`/`fmtp`.
//!
//! SDP 会话描述解析属性测试。
//!
//! 测试覆盖解析器对任意输入的鲁棒性、完整 SDP 往返、`o=`、`c=`、`m=` 与属性
//! 解析，包括 `rtpmap`/`fmtp`。

use cheetah_rtsp_core::{
    Sdp, SdpAttribute, SdpBuilder, SdpConnection, SdpError, SdpMedia, SdpMediaBuilder, SdpOrigin,
};
use proptest::prelude::*;

/// Generate a valid SDP session name without leading/trailing spaces.
///
/// 生成不含前导/后缀空格的有效 SDP 会话名。
fn valid_session_name() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9_-]{1,30}").expect("valid session name regex")
}

/// Generate a valid originator username.
///
/// 生成有效 originator 用户名。
fn valid_username() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9_-]{1,20}").expect("valid username regex")
}

/// Generate a valid session id/version.
///
/// 生成有效 session id/version。
fn valid_session_id() -> impl Strategy<Value = String> {
    prop::string::string_regex("[0-9]{1,20}").expect("valid session id regex")
}

/// Generate a valid unicast IP address.
///
/// 生成有效单播 IP 地址。
fn valid_ip_address() -> impl Strategy<Value = String> {
    prop::string::string_regex("[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}")
        .expect("valid ip address regex")
}

/// Generate a valid SDP origin.
///
/// 生成有效 SDP origin。
fn valid_origin() -> impl Strategy<Value = SdpOrigin> {
    (
        valid_username(),
        valid_session_id(),
        valid_session_id(),
        valid_ip_address(),
    )
        .prop_map(
            |(username, session_id, session_version, address)| SdpOrigin {
                username,
                session_id,
                session_version,
                net_type: "IN".to_string(),
                addr_type: "IP4".to_string(),
                address,
            },
        )
}

/// Generate a valid SDP connection.
///
/// 生成有效 SDP connection。
fn valid_connection() -> impl Strategy<Value = SdpConnection> {
    valid_ip_address().prop_map(|address| SdpConnection {
        net_type: "IN".to_string(),
        addr_type: "IP4".to_string(),
        address,
    })
}

/// Generate a valid SDP media entry.
///
/// 生成有效 SDP media 条目。
fn valid_media() -> impl Strategy<Value = SdpMedia> {
    (
        prop_oneof![
            Just("video".to_string()),
            Just("audio".to_string()),
            Just("application".to_string()),
        ],
        1024..65000_u16,
        prop_oneof![
            Just("RTP/AVP".to_string()),
            Just("RTP/SAVP".to_string()),
            Just("UDP".to_string()),
        ],
        prop::collection::vec(0..128_u8, 1..5),
    )
        .prop_map(|(media_type, port, protocol, formats)| {
            let mut builder = SdpMediaBuilder::new(&media_type, port, &protocol);
            for format in formats {
                builder = builder.format(&format.to_string());
            }
            builder.build()
        })
}

/// Build a well-formed SDP description from a builder and an origin.
///
/// 使用构造器与 origin 构造格式良好的 SDP 描述。
fn build_sdp(
    origin: SdpOrigin,
    session_name: &str,
    connection: SdpConnection,
    media: &[SdpMedia],
) -> Sdp {
    let mut builder = SdpBuilder::new()
        .origin(origin)
        .session_name(session_name)
        .connection(connection)
        .timing(0, 0);
    for media_item in media {
        builder = builder.add_media(media_item.clone());
    }
    builder.build().expect("valid sdp")
}

/// Append a set of media attributes to an already-built SDP.
///
/// 将媒体属性追加到已构建的 SDP。
fn with_media_attributes(mut sdp: Sdp, payload: u8, encoding: &str, clock_rate: u32) -> Sdp {
    if let Some(media) = sdp.media.first_mut() {
        media.attributes.push(SdpAttribute::Rtpmap {
            payload_type: payload,
            encoding: encoding.to_string(),
            clock_rate,
            encoding_params: None,
        });
        media.attributes.push(SdpAttribute::Fmtp {
            payload_type: payload,
            parameters: "profile-level-id=42e01f".to_string(),
        });
    }
    sdp
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// The parser never panics on arbitrary input.
    ///
    /// 解析器对任意输入不 panic。
    #[test]
    fn test_sdp_parse_arbitrary_input(input in "\\PC{0,200}") {
        let _ = Sdp::parse(&input);
    }

    /// Well-formed SDP input round-trips the origin/session name/connection.
    ///
    /// 格式良好的 SDP 输入往返保持 origin/会话名/connection。
    #[test]
    fn test_sdp_roundtrip(
        origin in valid_origin(),
        session_name in valid_session_name(),
        connection in valid_connection(),
        media in prop::collection::vec(valid_media(), 1..3),
    ) {
        let sdp = build_sdp(origin.clone(), &session_name, connection.clone(), &media);
        let text = sdp.to_string();
        let parsed = Sdp::parse(&text).expect("parse generated sdp");

        prop_assert_eq!(parsed.origin.username, origin.username);
        prop_assert_eq!(parsed.origin.session_id, origin.session_id);
        prop_assert_eq!(parsed.origin.session_version, origin.session_version);
        prop_assert_eq!(parsed.origin.net_type, origin.net_type);
        prop_assert_eq!(parsed.origin.addr_type, origin.addr_type);
        prop_assert_eq!(parsed.origin.address, origin.address);
        prop_assert_eq!(parsed.session_name, session_name);
        prop_assert_eq!(
            parsed.connection.as_ref().map(|c| c.address.clone()),
            Some(connection.address)
        );
        prop_assert_eq!(parsed.media.len(), media.len());
    }

    /// Origin fields are individually extracted.
    ///
    /// Origin 字段被单独提取。
    #[test]
    fn test_sdp_origin_parsed(
        origin in valid_origin(),
        session_name in valid_session_name(),
        connection in valid_connection(),
    ) {
        let sdp = build_sdp(origin.clone(), &session_name, connection, &[]);
        let text = sdp.to_string();
        let parsed = Sdp::parse(&text).expect("parse sdp");

        prop_assert_eq!(parsed.origin.username, origin.username);
        prop_assert_eq!(parsed.origin.session_id, origin.session_id);
        prop_assert_eq!(parsed.origin.session_version, origin.session_version);
        prop_assert_eq!(parsed.origin.address, origin.address);
    }

    /// Media entries preserve type, port, protocol, and format list.
    ///
    /// Media 条目保持 type、port、protocol 与 format 列表。
    #[test]
    fn test_sdp_media_line(
        media in valid_media(),
    ) {
        let origin = SdpOrigin {
            username: "-".to_string(),
            session_id: "0".to_string(),
            session_version: "0".to_string(),
            net_type: "IN".to_string(),
            addr_type: "IP4".to_string(),
            address: "127.0.0.1".to_string(),
        };
        let connection = SdpConnection {
            net_type: "IN".to_string(),
            addr_type: "IP4".to_string(),
            address: "127.0.0.1".to_string(),
        };
        let sdp = build_sdp(origin, "test", connection, &[media.clone()]);
        let text = sdp.to_string();
        let parsed = Sdp::parse(&text).expect("parse sdp");

        prop_assert_eq!(parsed.media.len(), 1);
        let parsed_media = &parsed.media[0];
        prop_assert_eq!(&parsed_media.media_type, &media.media_type);
        prop_assert_eq!(parsed_media.port, media.port);
        prop_assert_eq!(&parsed_media.protocol, &media.protocol);
        prop_assert_eq!(&parsed_media.formats, &media.formats);
    }

    /// `rtpmap` and `fmtp` attributes are parsed and associated with the correct payload.
    ///
    /// `rtpmap` 与 `fmtp` 属性被正确解析并关联到对应 payload。
    #[test]
    fn test_sdp_rtpmap_fmtp_parsed(
        payload in 0..96_u8,
        encoding in prop_oneof![Just("H264"), Just("H265"), Just("AAC"), Just("PCMU")],
        clock_rate in prop_oneof![Just(90000_u32), Just(44100), Just(8000), Just(48000)],
    ) {
        let media = SdpMediaBuilder::new("video", 0, "RTP/AVP")
            .format(&payload.to_string())
            .build();
        let origin = SdpOrigin {
            username: "-".to_string(),
            session_id: "0".to_string(),
            session_version: "0".to_string(),
            net_type: "IN".to_string(),
            addr_type: "IP4".to_string(),
            address: "127.0.0.1".to_string(),
        };
        let connection = SdpConnection {
            net_type: "IN".to_string(),
            addr_type: "IP4".to_string(),
            address: "127.0.0.1".to_string(),
        };
        let sdp = build_sdp(origin, "test", connection, &[media]);
        let sdp = with_media_attributes(sdp, payload, &encoding, clock_rate);
        let text = sdp.to_string();

        let parsed = Sdp::parse(&text).expect("parse sdp with rtpmap/fmtp");
        prop_assert_eq!(parsed.media.len(), 1);
        let parsed_media = &parsed.media[0];
        let format_strings: Vec<String> = vec![payload.to_string()];
        prop_assert_eq!(&parsed_media.formats, &format_strings);

        let rtpmap_attr = parsed_media.attributes.iter().find_map(|attr| {
            if let SdpAttribute::Rtpmap {
                payload_type, encoding, clock_rate, ..
            } = attr
            {
                Some((*payload_type, encoding.clone(), *clock_rate))
            } else {
                None
            }
        });
        prop_assert_eq!(rtpmap_attr, Some((payload, encoding.to_string(), clock_rate)));

        let fmtp_attr = parsed_media.attributes.iter().find_map(|attr| {
            if let SdpAttribute::Fmtp { payload_type, parameters } = attr {
                Some((*payload_type, parameters.clone()))
            } else {
                None
            }
        });
        prop_assert_eq!(
            fmtp_attr,
            Some((payload, "profile-level-id=42e01f".to_string()))
        );
    }
}

/// Invalid SDP inputs must be rejected explicitly instead of returning a partial success.
///
/// 非法 SDP 输入必须被显式拒绝，不能返回部分成功。
#[test]
fn test_sdp_parse_invalid_data() {
    assert!(matches!(
        Sdp::parse(""),
        Err(SdpError::MissingRequiredField { field: "o" })
    ));
    assert!(matches!(
        Sdp::parse("v=1\r\n"),
        Err(SdpError::MissingRequiredField { field: "o" })
    ));
    assert!(Sdp::parse("v=0\r\n").is_err());
    assert!(Sdp::parse("invalid").is_err());
}
