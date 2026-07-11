//! Property-based round-trip tests for RTMP command messages.
//!
//! Each `RtmpCommand` variant is converted to a generic `RtmpMessage::Command`,
//! serialized to AMF0 values, and then parsed back. The tests also verify that
//! malformed commands (wrong transaction id, missing arguments, invalid types)
//! are rejected by the parser.
//!
//! RTMP 命令消息的属性测试往返测试。
//!
//! 每个 `RtmpCommand` 变体被转换为通用 `RtmpMessage::Command`，序列化为 AMF0 值后再解析回来。
//! 测试还校验解析器会拒绝格式错误的命令（错误事务 id、缺失参数、无效类型）。

use cheetah_rtmp_core::{
    Amf0Value, AmfValue, RtmpCommand, RtmpMessage, RtmpMessageStreamId, TransactionId,
};
use proptest::prelude::*;

// =============================================================================
// Strategy definitions
// =============================================================================

/// Generate a short ASCII string for command fields.
///
/// 生成用于命令字段的短 ASCII 字符串。
fn arb_small_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_./-]{0,20}".prop_map(|s| s.to_string())
}

/// Generate a `TransactionId`.
///
/// 生成 `TransactionId`。
fn arb_transaction_id() -> impl Strategy<Value = TransactionId> {
    (0i64..=1000i64).prop_map(|v| TransactionId::from_f64(v as f64))
}

/// Generate a `RtmpMessageStreamId`.
///
/// 生成 `RtmpMessageStreamId`。
fn arb_stream_id() -> impl Strategy<Value = RtmpMessageStreamId> {
    any::<u32>().prop_map(RtmpMessageStreamId::new)
}

/// Generate a small subset of `AmfValue` values for command arguments.
///
/// 生成用于命令参数的小范围 `AmfValue` 值。
fn arb_amf_value() -> impl Strategy<Value = AmfValue> {
    prop_oneof![
        Just(AmfValue::Amf0(Amf0Value::Null)),
        arb_small_string().prop_map(|s| AmfValue::Amf0(Amf0Value::String(s))),
        prop::num::f64::NORMAL.prop_map(|n| AmfValue::Amf0(Amf0Value::Number(n))),
    ]
}

// =============================================================================
// Round-trip tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Verify that `RtmpCommand::Connect` round-trips through message encoding.
    ///
    /// 校验 `RtmpCommand::Connect` 通过消息编码往返。
    #[test]
    fn connect_roundtrip(
        app in arb_small_string(),
        flash_ver in arb_small_string(),
        tc_url in arb_small_string(),
    ) {
        let command = RtmpCommand::Connect(cheetah_rtmp_core::RtmpConnectCommand {
            app: app.clone(),
            flash_ver: flash_ver.clone(),
            tc_url: tc_url.clone(),
        });

        let message = command.into_pcm_message().expect("connect should encode");
        let RtmpMessage::Command { name, transaction_id, object, args, .. } = message else {
            return Err(TestCaseError::fail("expected command message"));
        };

        prop_assert_eq!(name.as_str(), "connect");
        prop_assert_eq!(transaction_id, TransactionId::CONNECT);
        prop_assert!(args.is_empty());

        let decoded = RtmpCommand::from_message(&name, transaction_id, object, args)
            .expect("connect should decode");
        match decoded {
            RtmpCommand::Connect(cmd) => {
                prop_assert_eq!(cmd.app, app);
                prop_assert_eq!(cmd.flash_ver, flash_ver);
                prop_assert_eq!(cmd.tc_url, tc_url);
            }
            _ => return Err(TestCaseError::fail("expected connect command")),
        }
    }

    /// Verify that `RtmpCommand::CreateStream` round-trips.
    ///
    /// 校验 `RtmpCommand::CreateStream` 往返。
    #[test]
    fn create_stream_roundtrip(transaction_id in arb_transaction_id()) {
        let command = RtmpCommand::CreateStream(cheetah_rtmp_core::RtmpCreateStreamCommand {
            transaction_id,
        });

        let message = command.into_pcm_message().expect("createStream should encode");
        let RtmpMessage::Command { name, transaction_id, object, args, .. } = message else {
            return Err(TestCaseError::fail("expected command message"));
        };

        prop_assert_eq!(name.as_str(), "createStream");
        prop_assert!(args.is_empty());

        let decoded = RtmpCommand::from_message(&name, transaction_id, object, args)
            .expect("createStream should decode");
        match decoded {
            RtmpCommand::CreateStream(cmd) => {
                prop_assert_eq!(cmd.transaction_id, transaction_id);
            }
            _ => return Err(TestCaseError::fail("expected createStream command")),
        }
    }

    /// Verify that `RtmpCommand::Publish` round-trips.
    ///
    /// 校验 `RtmpCommand::Publish` 往返。
    #[test]
    fn publish_roundtrip(
        transaction_id in arb_transaction_id(),
        stream_name in arb_small_string(),
    ) {
        let command = RtmpCommand::Publish(cheetah_rtmp_core::RtmpPublishCommand {
            transaction_id,
            stream_name: stream_name.clone(),
        });

        let message = command.into_pcm_message().expect("publish should encode");
        let RtmpMessage::Command { name, transaction_id, object, args, .. } = message else {
            return Err(TestCaseError::fail("expected command message"));
        };

        prop_assert_eq!(name.as_str(), "publish");

        let decoded = RtmpCommand::from_message(&name, transaction_id, object, args)
            .expect("publish should decode");
        match decoded {
            RtmpCommand::Publish(cmd) => {
                prop_assert_eq!(cmd.transaction_id, transaction_id);
                prop_assert_eq!(cmd.stream_name, stream_name);
            }
            _ => return Err(TestCaseError::fail("expected publish command")),
        }
    }

    /// Verify that `RtmpCommand::Play` round-trips.
    ///
    /// 校验 `RtmpCommand::Play` 往返。
    #[test]
    fn play_roundtrip(
        transaction_id in arb_transaction_id(),
        stream_name in arb_small_string(),
        start in prop::num::f64::NORMAL,
    ) {
        let command = RtmpCommand::Play(cheetah_rtmp_core::RtmpPlayCommand {
            transaction_id,
            stream_name: stream_name.clone(),
            start,
        });

        let message = command.into_pcm_message().expect("play should encode");
        let RtmpMessage::Command { name, transaction_id, object, args, .. } = message else {
            return Err(TestCaseError::fail("expected command message"));
        };

        prop_assert_eq!(name.as_str(), "play");

        let decoded = RtmpCommand::from_message(&name, transaction_id, object, args)
            .expect("play should decode");
        match decoded {
            RtmpCommand::Play(cmd) => {
                prop_assert_eq!(cmd.transaction_id, transaction_id);
                prop_assert_eq!(cmd.stream_name, stream_name);
                prop_assert_eq!(cmd.start, start);
            }
            _ => return Err(TestCaseError::fail("expected play command")),
        }
    }

    /// Verify that `RtmpCommand::DeleteStream` round-trips.
    ///
    /// 校验 `RtmpCommand::DeleteStream` 往返。
    #[test]
    fn delete_stream_roundtrip(
        transaction_id in arb_transaction_id(),
        stream_id in arb_stream_id(),
    ) {
        let command = RtmpCommand::DeleteStream(cheetah_rtmp_core::RtmpDeleteStreamCommand {
            transaction_id,
            stream_id,
        });

        let message = command.into_pcm_message().expect("deleteStream should encode");
        let RtmpMessage::Command { name, transaction_id, object, args, .. } = message else {
            return Err(TestCaseError::fail("expected command message"));
        };

        prop_assert_eq!(name.as_str(), "deleteStream");

        let decoded = RtmpCommand::from_message(&name, transaction_id, object, args)
            .expect("deleteStream should decode");
        match decoded {
            RtmpCommand::DeleteStream(cmd) => {
                prop_assert_eq!(cmd.transaction_id, transaction_id);
                prop_assert_eq!(cmd.stream_id, stream_id);
            }
            _ => return Err(TestCaseError::fail("expected deleteStream command")),
        }
    }

    /// Verify that `RtmpCommand::GetStreamLength` round-trips.
    ///
    /// 校验 `RtmpCommand::GetStreamLength` 往返。
    #[test]
    fn get_stream_length_roundtrip(
        transaction_id in arb_transaction_id(),
        stream_name in arb_small_string(),
    ) {
        let command = RtmpCommand::GetStreamLength(cheetah_rtmp_core::RtmpGetStreamLengthCommand {
            transaction_id,
            stream_name: stream_name.clone(),
        });

        let message = command.into_pcm_message().expect("getStreamLength should encode");
        let RtmpMessage::Command { name, transaction_id, object, args, .. } = message else {
            return Err(TestCaseError::fail("expected command message"));
        };

        prop_assert_eq!(name.as_str(), "getStreamLength");

        let decoded = RtmpCommand::from_message(&name, transaction_id, object, args)
            .expect("getStreamLength should decode");
        match decoded {
            RtmpCommand::GetStreamLength(cmd) => {
                prop_assert_eq!(cmd.transaction_id, transaction_id);
                prop_assert_eq!(cmd.stream_name, stream_name);
            }
            _ => return Err(TestCaseError::fail("expected getStreamLength command")),
        }
    }

    /// Verify that `RtmpCommand::Result` round-trips.
    ///
    /// 校验 `RtmpCommand::Result` 往返。
    #[test]
    fn result_roundtrip(
        transaction_id in arb_transaction_id(),
        properties in arb_amf_value(),
        information in arb_amf_value(),
    ) {
        let command = RtmpCommand::Result(cheetah_rtmp_core::RtmpResultCommand {
            transaction_id,
            properties: properties.clone(),
            information: information.clone(),
        });

        let message = command.into_pcm_message().expect("_result should encode");
        let RtmpMessage::Command { name, transaction_id, object, args, .. } = message else {
            return Err(TestCaseError::fail("expected command message"));
        };

        prop_assert_eq!(name.as_str(), "_result");

        let decoded = RtmpCommand::from_message(&name, transaction_id, object, args)
            .expect("_result should decode");
        match decoded {
            RtmpCommand::Result(cmd) => {
                prop_assert_eq!(cmd.transaction_id, transaction_id);
                prop_assert_eq!(cmd.properties, properties);
                prop_assert_eq!(cmd.information, information);
            }
            _ => return Err(TestCaseError::fail("expected _result command")),
        }
    }

    /// Verify that `connect` with a transaction id other than 1 is rejected.
    ///
    /// 校验事务 id 不为 1 的 `connect` 被拒绝。
    #[test]
    fn connect_invalid_transaction_id_rejected(
        transaction_id in (0i64..=10i64).prop_filter("not connect id", |v| *v != 1),
        app in arb_small_string(),
        flash_ver in arb_small_string(),
        tc_url in arb_small_string(),
    ) {
        let object = AmfValue::amf0_object([
            ("app", Amf0Value::String(app)),
            ("flashVer", Amf0Value::String(flash_ver)),
            ("tcUrl", Amf0Value::String(tc_url)),
        ]);
        let result = RtmpCommand::from_message(
            "connect",
            TransactionId::from_f64(transaction_id as f64),
            object,
            vec![],
        );
        prop_assert!(result.is_err());
    }

    /// Verify that `publish` with no stream name argument is rejected.
    ///
    /// 校验缺少流名称参数的 `publish` 被拒绝。
    #[test]
    fn publish_missing_args_rejected(transaction_id in arb_transaction_id()) {
        let result = RtmpCommand::from_message(
            "publish",
            transaction_id,
            AmfValue::Amf0(Amf0Value::Null),
            vec![],
        );
        prop_assert!(result.is_err());
    }

    /// Verify that `publish` with a type other than "live" is rejected.
    ///
    /// 校验发布类型不是 "live" 的 `publish` 被拒绝。
    #[test]
    fn publish_invalid_type_rejected(
        transaction_id in arb_transaction_id(),
        stream_name in arb_small_string(),
        publish_type in arb_small_string(),
    ) {
        prop_assume!(publish_type != "live");
        let result = RtmpCommand::from_message(
            "publish",
            transaction_id,
            AmfValue::Amf0(Amf0Value::Null),
            vec![
                AmfValue::Amf0(Amf0Value::String(stream_name)),
                AmfValue::Amf0(Amf0Value::String(publish_type)),
            ],
        );
        prop_assert!(result.is_err());
    }

    /// Verify that `play` with a missing `start` argument is rejected.
    ///
    /// 校验缺少 `start` 参数的 `play` 被拒绝。
    #[test]
    fn play_missing_start_rejected(
        transaction_id in arb_transaction_id(),
        stream_name in arb_small_string(),
    ) {
        let result = RtmpCommand::from_message(
            "play",
            transaction_id,
            AmfValue::Amf0(Amf0Value::Null),
            vec![AmfValue::Amf0(Amf0Value::String(stream_name))],
        );
        prop_assert!(result.is_err());
    }

    /// Verify that `play` with a non-numeric `start` argument is rejected.
    ///
    /// 校验 `start` 参数非数字的 `play` 被拒绝。
    #[test]
    fn play_invalid_start_type_rejected(
        transaction_id in arb_transaction_id(),
        stream_name in arb_small_string(),
    ) {
        let result = RtmpCommand::from_message(
            "play",
            transaction_id,
            AmfValue::Amf0(Amf0Value::Null),
            vec![
                AmfValue::Amf0(Amf0Value::String(stream_name)),
                AmfValue::Amf0(Amf0Value::Null),
            ],
        );
        prop_assert!(result.is_err());
    }

    /// Verify that `deleteStream` with a missing stream id argument is rejected.
    ///
    /// 校验缺少流 id 参数的 `deleteStream` 被拒绝。
    #[test]
    fn delete_stream_missing_id_rejected(transaction_id in arb_transaction_id()) {
        let result = RtmpCommand::from_message(
            "deleteStream",
            transaction_id,
            AmfValue::Amf0(Amf0Value::Null),
            vec![],
        );
        prop_assert!(result.is_err());
    }
}
