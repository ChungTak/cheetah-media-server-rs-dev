//! Property-based tests for RTSP transport, range, and method parsing.
//!
//! These tests verify `RtspTransport` parse/display round-trips, comma-separated
//! transport lists, SMPTE/NPT range round-trips, and extension method parsing.
//!
//! RTSP 传输、区间与方法解析属性测试。
//!
//! 这些测试验证 `RtspTransport` 解析/显示往返、逗号分隔的 transport 列表、
//! SMPTE/NPT 区间往返以及扩展方法解析。

use cheetah_rtsp_core::{
    NptRange, NptTime, RtspMethod, RtspRange, RtspTransport, SmpteRange, SmpteTime, SmpteType,
};
use proptest::prelude::*;

/// Generate a valid transport protocol string.
///
/// 生成有效 transport 协议字符串。
fn valid_transport_protocol() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("RTP/AVP".to_string()),
        Just("RTP/AVP/TCP".to_string()),
        Just("RTP/SAVP".to_string()),
    ]
}

/// Generate a valid `RtspTransport` structure.
///
/// 生成有效 `RtspTransport` 结构。
fn valid_transport() -> impl Strategy<Value = RtspTransport> {
    (
        valid_transport_protocol(),
        any::<bool>(),
        prop::option::of((0..128_u8, 0..128_u8)),
        prop::option::of((1024..65000_u16, 1024..65000_u16)),
        prop::option::of((1024..65000_u16, 1024..65000_u16)),
        prop::option::of(any::<u32>()),
        prop::option::of(prop_oneof![
            Just("PLAY".to_string()),
            Just("RECORD".to_string()),
        ]),
        prop::option::of(
            prop::string::string_regex("[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}")
                .expect("valid destination regex"),
        ),
        prop::option::of(
            prop::string::string_regex("[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}")
                .expect("valid source regex"),
        ),
        prop::option::of(1..255_u8),
    )
        .prop_map(
            |(
                protocol,
                unicast,
                interleaved,
                client_port,
                server_port,
                ssrc,
                mode,
                destination,
                source,
                ttl,
            )| RtspTransport {
                protocol,
                unicast,
                interleaved,
                client_port,
                server_port,
                ssrc,
                mode,
                destination,
                source,
                ttl,
                ..RtspTransport::default()
            },
        )
}

/// Generate a valid SMPTE type.
///
/// 生成有效 SMPTE 类型。
fn valid_smpte_type() -> impl Strategy<Value = SmpteType> {
    prop_oneof![
        Just(SmpteType::Smpte),
        Just(SmpteType::Smpte30Drop),
        Just(SmpteType::Smpte25),
    ]
}

/// Generate a valid SMPTE timestamp.
///
/// 生成有效 SMPTE 时间戳。
fn valid_smpte_time() -> impl Strategy<Value = SmpteTime> {
    (0..24_u8, 0..60_u8, 0..60_u8, 0..30_u8).prop_map(|(hours, minutes, seconds, frames)| {
        SmpteTime {
            hours,
            minutes,
            seconds,
            frames,
            subframes: None,
        }
    })
}

/// Generate a valid extension method name (excludes standard methods).
///
/// 生成有效扩展方法名（排除标准方法）。
fn valid_extension_method() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Z][A-Z_]{2,20}")
        .expect("valid extension method regex")
        .prop_filter("not a standard method", |name| {
            !matches!(
                name.as_str(),
                "OPTIONS"
                    | "DESCRIBE"
                    | "ANNOUNCE"
                    | "SETUP"
                    | "PLAY"
                    | "PAUSE"
                    | "TEARDOWN"
                    | "GET_PARAMETER"
                    | "SET_PARAMETER"
                    | "REDIRECT"
                    | "RECORD"
            )
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Transport header parse/display round-trip.
    ///
    /// Transport 头解析/显示往返。
    #[test]
    fn test_transport_roundtrip(transport in valid_transport()) {
        let header = transport.to_header();
        let parsed = RtspTransport::parse(&header).expect("parse transport header");

        prop_assert_eq!(parsed.protocol, transport.protocol);
        prop_assert_eq!(parsed.unicast, transport.unicast);
        prop_assert_eq!(parsed.interleaved, transport.interleaved);
        prop_assert_eq!(parsed.client_port, transport.client_port);
        prop_assert_eq!(parsed.server_port, transport.server_port);
        prop_assert_eq!(parsed.ssrc, transport.ssrc);
        prop_assert_eq!(parsed.mode, transport.mode);
        prop_assert_eq!(parsed.destination, transport.destination);
        prop_assert_eq!(parsed.source, transport.source);
        prop_assert_eq!(parsed.ttl, transport.ttl);
    }

    /// Multiple comma-separated Transport values parse correctly.
    ///
    /// 多个逗号分隔的 Transport 值正确解析。
    #[test]
    fn test_transport_parse_multiple(
        t1 in valid_transport(),
        t2 in valid_transport(),
    ) {
        let header = format!("{}, {}", t1.to_header(), t2.to_header());
        let parsed = RtspTransport::parse_multiple(&header).expect("parse multiple transport");

        prop_assert_eq!(parsed.len(), 2);
        prop_assert_eq!(&parsed[0].protocol, &t1.protocol);
        prop_assert_eq!(&parsed[1].protocol, &t2.protocol);
    }

    /// SMPTE type is preserved through parse/display round-trip.
    ///
    /// SMPTE 类型在解析/显示往返后保持一致。
    #[test]
    fn test_smpte_type_roundtrip(
        smpte_type in valid_smpte_type(),
        start in valid_smpte_time(),
    ) {
        let range = RtspRange::Smpte(SmpteRange {
            smpte_type,
            start,
            end: None,
        });

        let text = range.to_string();
        let parsed = RtspRange::parse(&text).expect("parse range");

        if let RtspRange::Smpte(smpte) = parsed {
            prop_assert_eq!(smpte.smpte_type, smpte_type);
        } else {
            prop_assert!(false, "expected smpte range");
        }
    }

    /// NPT range parse/display round-trip.
    ///
    /// NPT 区间解析/显示往返。
    #[test]
    fn test_npt_roundtrip(seconds in 0.0f64..86400.0) {
        let range = RtspRange::Npt(NptRange {
            start: NptTime::Seconds(seconds),
            end: None,
        });

        let text = range.to_string();
        let parsed = RtspRange::parse(&text).expect("parse npt range");

        if let RtspRange::Npt(npt) = parsed {
            if let NptTime::Seconds(parsed_seconds) = npt.start {
                prop_assert!((parsed_seconds - seconds).abs() < 0.001);
            } else {
                prop_assert!(false, "expected seconds start time");
            }
        } else {
            prop_assert!(false, "expected npt range");
        }
    }

    /// Extension methods round-trip through parse/as_str/display.
    ///
    /// 扩展方法通过 parse/as_str/display 保持往返。
    #[test]
    fn test_extension_method_roundtrip(name in valid_extension_method()) {
        let method: RtspMethod = name.parse().expect("infallible parse");

        if let RtspMethod::Extension(ref ext_name) = method {
            prop_assert_eq!(ext_name, &name);
            prop_assert_eq!(method.as_str(), name.as_str());
            prop_assert_eq!(method.to_string(), name);
        } else {
            prop_assert!(false, "expected extension method");
        }
    }

    /// Standard methods are case-sensitive: uppercase matches a standard method,
    /// lowercase becomes an extension method.
    ///
    /// 标准方法大小写敏感：大写命中标准方法，小写保持扩展方法。
    #[test]
    fn test_standard_method_case_sensitive(
        method_name in prop_oneof![
            Just("OPTIONS"),
            Just("DESCRIBE"),
            Just("ANNOUNCE"),
            Just("SETUP"),
            Just("PLAY"),
            Just("PAUSE"),
            Just("TEARDOWN"),
            Just("GET_PARAMETER"),
            Just("SET_PARAMETER"),
            Just("REDIRECT"),
            Just("RECORD"),
        ],
    ) {
        let standard: RtspMethod = method_name.parse().expect("infallible parse");
        prop_assert!(!matches!(standard, RtspMethod::Extension(_)));

        let lower = method_name.to_ascii_lowercase();
        let lower_method: RtspMethod = lower.parse().expect("infallible parse");
        prop_assert!(matches!(lower_method, RtspMethod::Extension(ref value) if value == &lower));
    }
}
