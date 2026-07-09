// 来源: vendor-ref/rtsp-rs/pbt/tests/pbt_rtsp.rs

use cheetah_rtsp_core::{
    NptRange, NptTime, RtspMethod, RtspRange, RtspTransport, SmpteRange, SmpteTime, SmpteType,
};
use proptest::prelude::*;

/// 生成有效的 Transport 协议字段。
fn valid_transport_protocol() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("RTP/AVP".to_string()),
        Just("RTP/AVP/TCP".to_string()),
        Just("RTP/SAVP".to_string()),
    ]
}

/// 生成有效的 Transport 头结构。
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

/// 生成有效的 SMPTE 类型。
fn valid_smpte_type() -> impl Strategy<Value = SmpteType> {
    prop_oneof![
        Just(SmpteType::Smpte),
        Just(SmpteType::Smpte30Drop),
        Just(SmpteType::Smpte25),
    ]
}

/// 生成有效的 SMPTE 时间戳。
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

/// 生成有效的扩展 RTSP 方法名（排除标准方法）。
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

    /// Transport 头应可完成 to_header/parse roundtrip。
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

    /// 多个 Transport 头值应可按逗号分隔正确解析。
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

    /// SMPTE 类型应在 parse/display roundtrip 后保持一致。
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

    /// NPT 区间应可完成 parse/display roundtrip。
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

    /// 扩展方法应可保持 parse/as_str/display roundtrip。
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

    /// 标准方法应保持大小写敏感：大写命中标准方法，小写保持扩展方法。
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
