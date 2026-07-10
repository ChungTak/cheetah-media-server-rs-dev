use crate::amf::{AmfValue, AmfVersion};
use crate::amf0::Amf0Value;
use crate::error::Error;
use crate::message::{RtmpMessage, RtmpMessageHeader, RtmpMessageStreamId};
use crate::prelude::*;
use crate::timestamp::RtmpTimestamp;

// [NOTE]
// 从格式（AMF）的表现力来说应该用 f64，
// 但 RTMP 规范不推荐使用小数，而且实际上也不会用到，
// 因此内部用整数来保存
/// `TransactionId` data structure.
/// `TransactionId` 数据结构.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TransactionId(i64);

impl TransactionId {
    pub const ON_STATUS: Self = Self(0);
    pub const CONNECT: Self = Self(1);
    pub const NON_RESERVED_START: Self = Self(2);

    /// Creates `f64` from input.
    /// 创建 `f64` 来自 输入.
    pub fn from_f64(id: f64) -> Self {
        Self(round_f64_to_i64(id))
    }

    /// `get` function.
    /// `get` 函数.
    pub const fn get(self) -> i64 {
        self.0
    }

    /// `increment` function.
    /// `increment` 函数.
    pub fn increment(&mut self) {
        self.0 += 1
    }
}

fn round_f64_to_i64(value: f64) -> i64 {
    let truncated = value as i64;
    let fractional = value - truncated as f64;
    if value >= 0.0 {
        if fractional >= 0.5 {
            truncated.saturating_add(1)
        } else {
            truncated
        }
    } else if fractional <= -0.5 {
        truncated.saturating_sub(1)
    } else {
        truncated
    }
}

/// `RtmpCommand` enumeration.
/// `RtmpCommand` 枚举.
#[derive(Debug, Clone)]
pub enum RtmpCommand {
    /// `Connect` variant.
    /// `Connect` 变体.
    Connect(RtmpConnectCommand),
    /// `CreateStream` variant.
    /// `CreateStream` 变体.
    CreateStream(RtmpCreateStreamCommand),
    /// `Publish` variant.
    /// `Publish` 变体.
    Publish(RtmpPublishCommand),
    /// `Play` variant.
    /// `Play` 变体.
    Play(RtmpPlayCommand),
    /// `DeleteStream` variant.
    /// `DeleteStream` 变体.
    DeleteStream(RtmpDeleteStreamCommand),
    /// `GetStreamLength` variant.
    /// `GetStreamLength` 变体.
    GetStreamLength(RtmpGetStreamLengthCommand),
    /// `Result` variant.
    /// `Result` 变体.
    Result(RtmpResultCommand),
    /// `OnStatus` variant.
    /// `OnStatus` 变体.
    OnStatus(RtmpOnStatusCommand),
    /// `Ignore` variant.
    /// `Ignore` 变体.
    Ignore {
        name: String,

        // 以下字段仅用于调试显示，不会在代码主体中引用
        object: AmfValue,
        args: Vec<AmfValue>,
    },
}

impl RtmpCommand {
    /// `name` function.
    /// `name` 函数.
    pub fn name(&self) -> &str {
        match self {
            RtmpCommand::Connect(_) => "connect",
            RtmpCommand::CreateStream(_) => "createStream",
            RtmpCommand::Publish(_) => "publish",
            RtmpCommand::Play(_) => "play",
            RtmpCommand::DeleteStream(_) => "deleteStream",
            RtmpCommand::GetStreamLength(_) => "getStreamLength",
            RtmpCommand::Result(_) => "_result",
            RtmpCommand::OnStatus(_) => "onStatus",
            RtmpCommand::Ignore { name, .. } => name,
        }
    }

    /// `into_message` function.
    /// `into_message` 函数.
    pub fn into_message(self, header: RtmpMessageHeader) -> Result<RtmpMessage, Error> {
        match self {
            RtmpCommand::Ignore { .. } => {
                // 到这里是非预期情况（实现 bug）
                Err(Error::invalid_state("BUG"))
            }
            RtmpCommand::OnStatus(cmd) => {
                let mut pairs = vec![
                    ("level".to_string(), Amf0Value::String(cmd.level)),
                    ("code".to_string(), Amf0Value::String(cmd.code)),
                ];

                if let Some(description) = cmd.description {
                    pairs.push(("description".to_string(), Amf0Value::String(description)));
                }

                if let Some(details) = cmd.details {
                    pairs.push(("details".to_string(), Amf0Value::String(details)));
                }

                let status_object = AmfValue::Amf0(Amf0Value::Object {
                    class_name: None,
                    entries: pairs
                        .into_iter()
                        .map(|(k, v)| crate::amf::Pair { key: k, value: v })
                        .collect(),
                });

                Ok(RtmpMessage::Command {
                    header,
                    amf_version: AmfVersion::Amf0,
                    name: "onStatus".to_string(),
                    transaction_id: TransactionId::ON_STATUS,
                    object: AmfValue::Amf0(Amf0Value::Null),
                    args: vec![status_object],
                })
            }
            RtmpCommand::Connect(cmd) => {
                let object = AmfValue::amf0_object([
                    ("app", Amf0Value::String(cmd.app)),
                    ("flashVer", Amf0Value::String(cmd.flash_ver)),
                    ("tcUrl", Amf0Value::String(cmd.tc_url)),
                ]);
                Ok(RtmpMessage::Command {
                    header,
                    amf_version: AmfVersion::Amf0,
                    name: "connect".to_string(),
                    transaction_id: TransactionId::CONNECT,
                    object,
                    args: vec![],
                })
            }
            RtmpCommand::CreateStream(cmd) => Ok(RtmpMessage::Command {
                header,
                amf_version: AmfVersion::Amf0,
                name: "createStream".to_string(),
                transaction_id: cmd.transaction_id,
                object: AmfValue::Amf0(Amf0Value::Null),
                args: vec![],
            }),
            RtmpCommand::Publish(cmd) => Ok(RtmpMessage::Command {
                header,
                amf_version: AmfVersion::Amf0,
                name: "publish".to_string(),
                transaction_id: cmd.transaction_id,
                object: AmfValue::Amf0(Amf0Value::Null),
                args: vec![
                    AmfValue::Amf0(Amf0Value::String(cmd.stream_name)),
                    AmfValue::Amf0(Amf0Value::String("live".to_owned())), // publish_type
                ],
            }),
            RtmpCommand::Play(cmd) => Ok(RtmpMessage::Command {
                header,
                amf_version: AmfVersion::Amf0,
                name: "play".to_string(),
                transaction_id: cmd.transaction_id,
                object: AmfValue::Amf0(Amf0Value::Null),
                args: vec![
                    AmfValue::Amf0(Amf0Value::String(cmd.stream_name)),
                    AmfValue::Amf0(Amf0Value::Number(cmd.start)),
                ],
            }),
            RtmpCommand::DeleteStream(cmd) => Ok(RtmpMessage::Command {
                header,
                amf_version: AmfVersion::Amf0,
                name: "deleteStream".to_string(),
                transaction_id: cmd.transaction_id,
                object: AmfValue::Amf0(Amf0Value::Null),
                args: vec![AmfValue::Amf0(
                    Amf0Value::Number(cmd.stream_id.get() as f64),
                )],
            }),
            RtmpCommand::GetStreamLength(cmd) => Ok(RtmpMessage::Command {
                header,
                amf_version: AmfVersion::Amf0,
                name: "getStreamLength".to_string(),
                transaction_id: cmd.transaction_id,
                object: AmfValue::Amf0(Amf0Value::Null),
                args: vec![AmfValue::Amf0(Amf0Value::String(cmd.stream_name))],
            }),
            RtmpCommand::Result(cmd) => Ok(RtmpMessage::Command {
                header,
                amf_version: AmfVersion::Amf0,
                name: "_result".to_string(),
                transaction_id: cmd.transaction_id,
                object: cmd.properties,
                args: vec![cmd.information],
            }),
        }
    }

    /// `into_pcm_message` function.
    /// `into_pcm_message` 函数.
    pub fn into_pcm_message(self) -> Result<RtmpMessage, Error> {
        self.into_message(RtmpMessageHeader {
            stream_id: RtmpMessageStreamId::PCM,
            timestamp: RtmpTimestamp::ZERO,
        })
    }

    /// Creates `message` from input.
    /// 创建 `message` 来自 输入.
    pub fn from_message(
        name: &str,
        transaction_id: TransactionId,
        object: AmfValue,
        args: Vec<AmfValue>,
    ) -> Result<Self, Error> {
        match name {
            "connect" => {
                RtmpConnectCommand::from_message(transaction_id, object).map(Self::Connect)
            }
            "createStream" => RtmpCreateStreamCommand::from_message(transaction_id, object)
                .map(Self::CreateStream),
            "publish" => {
                RtmpPublishCommand::from_message(transaction_id, object, args).map(Self::Publish)
            }
            "play" => RtmpPlayCommand::from_message(transaction_id, object, args).map(Self::Play),
            "deleteStream" => RtmpDeleteStreamCommand::from_message(transaction_id, object, args)
                .map(Self::DeleteStream),
            "getStreamLength" => {
                RtmpGetStreamLengthCommand::from_message(transaction_id, object, args)
                    .map(Self::GetStreamLength)
            }
            "_result" | "_error" => {
                RtmpResultCommand::from_message(transaction_id, object, args).map(Self::Result)
            }
            "onStatus" => {
                RtmpOnStatusCommand::from_message(transaction_id, object, args).map(Self::OnStatus)
            }
            _ => {
                // 在本 crate 中未显式处理的全部视为 Ignore
                Ok(Self::Ignore {
                    name: name.to_string(),
                    object,
                    args,
                })
            }
        }
    }
}

/// `RtmpConnectCommand` data structure.
/// `RtmpConnectCommand` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpConnectCommand {
    /// `app` field of type `String`.
    /// `app` 字段，类型为 `String`.
    pub app: String,
    /// `flash_ver` field of type `String`.
    /// `flash_ver` 字段，类型为 `String`.
    pub flash_ver: String,
    /// `tc_url` field of type `String`.
    /// `tc_url` 字段，类型为 `String`.
    pub tc_url: String,
}

impl RtmpConnectCommand {
    fn from_message(transaction_id: TransactionId, object: AmfValue) -> Result<Self, Error> {
        if transaction_id != TransactionId::CONNECT {
            return Err(Error::invalid_data(format!(
                "invalid transaction ID for connect command: expected {}, got {}",
                TransactionId::CONNECT.get(),
                transaction_id.get()
            )));
        }

        let app = object
            .expect_object_member("app")?
            .expect_str()?
            .to_string();
        let flash_ver = object
            .expect_object_member("flashVer")
            .ok()
            .and_then(|v| v.expect_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "FMLE/3.0".to_string());
        let tc_url = object
            .expect_object_member("tcUrl")?
            .expect_str()?
            .to_string();
        Ok(Self {
            app,
            flash_ver,
            tc_url,
        })
    }

    /// `accept` function.
    /// `accept` 函数.
    pub fn accept(&self) -> Result<RtmpMessage, Error> {
        let properties = AmfValue::amf0_object([
            ("fmsVer", Amf0Value::String("FMS/4,5,0,297".to_string())),
            ("capabilities", Amf0Value::Number(255.0)),
            ("mode", Amf0Value::Number(1.0)),
        ]);
        let information = AmfValue::amf0_object([
            ("level", Amf0Value::String("status".to_string())),
            (
                "code",
                Amf0Value::String("NetConnection.Connect.Success".to_string()),
            ),
            (
                "description",
                Amf0Value::String("Connection succeeded.".to_string()),
            ),
            ("objectEncoding", Amf0Value::Number(0.0)),
        ]);
        let command = RtmpResultCommand {
            transaction_id: TransactionId::CONNECT,
            properties,
            information,
        };
        RtmpCommand::Result(command).into_pcm_message()
    }
}

/// `RtmpCreateStreamCommand` data structure.
/// `RtmpCreateStreamCommand` 数据结构.
#[derive(Debug, Clone)]
pub struct RtmpCreateStreamCommand {
    /// `transaction_id` field of type `TransactionId`.
    /// `transaction_id` 字段，类型为 `TransactionId`.
    pub transaction_id: TransactionId,
}

impl RtmpCreateStreamCommand {
    fn from_message(transaction_id: TransactionId, _object: AmfValue) -> Result<Self, Error> {
        Ok(Self { transaction_id })
    }
}

/// `RtmpPublishCommand` data structure.
/// `RtmpPublishCommand` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpPublishCommand {
    /// `transaction_id` field of type `TransactionId`.
    /// `transaction_id` 字段，类型为 `TransactionId`.
    pub transaction_id: TransactionId,
    /// `stream_name` field of type `String`.
    /// `stream_name` 字段，类型为 `String`.
    pub stream_name: String,
}

impl RtmpPublishCommand {
    fn from_message(
        transaction_id: TransactionId,
        _object: AmfValue,
        args: Vec<AmfValue>,
    ) -> Result<Self, Error> {
        let stream_name = args
            .first()
            .ok_or_else(|| Error::invalid_data("publish: missing stream name"))?
            .expect_str()?
            .to_string();
        let publish_type = args
            .get(1)
            .ok_or_else(|| Error::invalid_data("publish: missing publish type"))?
            .expect_str()?
            .to_string();
        if publish_type != "live" {
            return Err(Error::unsupported(format!(
                "unsupported publish type: {publish_type} (only 'live' is supported)",
            )));
        }

        Ok(Self {
            transaction_id,
            stream_name,
        })
    }

    /// `accept` function.
    /// `accept` 函数.
    pub fn accept(
        transaction_id: TransactionId,
        stream_id: RtmpMessageStreamId,
    ) -> Result<RtmpMessage, Error> {
        let properties = AmfValue::amf0_object(core::iter::empty());
        let information = AmfValue::amf0_object([
            ("level", Amf0Value::String("status".to_string())),
            (
                "code",
                Amf0Value::String("NetStream.Publish.Start".to_string()),
            ),
            (
                "description",
                Amf0Value::String("Publish succeeded.".to_string()),
            ),
        ]);
        let command = RtmpResultCommand {
            transaction_id,
            properties,
            information,
        };
        RtmpCommand::Result(command).into_message(RtmpMessageHeader {
            stream_id,
            timestamp: RtmpTimestamp::ZERO,
        })
    }
}

/// `RtmpPlayCommand` data structure.
/// `RtmpPlayCommand` 数据结构.
#[derive(Debug, Clone)]
pub struct RtmpPlayCommand {
    /// `transaction_id` field of type `TransactionId`.
    /// `transaction_id` 字段，类型为 `TransactionId`.
    pub transaction_id: TransactionId,
    /// `stream_name` field of type `String`.
    /// `stream_name` 字段，类型为 `String`.
    pub stream_name: String,
    /// `start` field of type `f64`.
    /// `start` 字段，类型为 `f64`.
    pub start: f64,
}

impl RtmpPlayCommand {
    fn from_message(
        transaction_id: TransactionId,
        _object: AmfValue,
        args: Vec<AmfValue>,
    ) -> Result<Self, Error> {
        let stream_name = args
            .first()
            .ok_or_else(|| Error::invalid_data("play: missing stream name"))?
            .expect_str()?
            .to_string();
        let start = args
            .get(1)
            .ok_or_else(|| Error::invalid_data("play: missing start position"))?
            .expect_number()?;
        Ok(Self {
            transaction_id,
            stream_name,
            start,
        })
    }

    /// `accept` function.
    /// `accept` 函数.
    pub fn accept(
        transaction_id: TransactionId,
        stream_id: RtmpMessageStreamId,
    ) -> Result<RtmpMessage, Error> {
        let properties = AmfValue::amf0_object(core::iter::empty());
        let information = AmfValue::amf0_object([
            ("level", Amf0Value::String("status".to_string())),
            (
                "code",
                Amf0Value::String("NetStream.Play.Start".to_string()),
            ),
            (
                "description",
                Amf0Value::String("Play succeeded.".to_string()),
            ),
        ]);
        let command = RtmpResultCommand {
            transaction_id,
            properties,
            information,
        };
        RtmpCommand::Result(command).into_message(RtmpMessageHeader {
            stream_id,
            timestamp: RtmpTimestamp::ZERO,
        })
    }
}

/// `RtmpDeleteStreamCommand` data structure.
/// `RtmpDeleteStreamCommand` 数据结构.
#[derive(Debug, Clone)]
pub struct RtmpDeleteStreamCommand {
    /// `transaction_id` field of type `TransactionId`.
    /// `transaction_id` 字段，类型为 `TransactionId`.
    pub transaction_id: TransactionId,
    /// `stream_id` field of type `RtmpMessageStreamId`.
    /// `stream_id` 字段，类型为 `RtmpMessageStreamId`.
    pub stream_id: RtmpMessageStreamId,
}

impl RtmpDeleteStreamCommand {
    fn from_message(
        transaction_id: TransactionId,
        _object: AmfValue,
        args: Vec<AmfValue>,
    ) -> Result<Self, Error> {
        let stream_id = args
            .first()
            .ok_or_else(|| Error::invalid_data("deleteStream: missing stream id"))?
            .expect_number()?;
        Ok(Self {
            transaction_id,
            stream_id: RtmpMessageStreamId::new(round_f64_to_i64(stream_id).max(0) as u32),
        })
    }
}

/// `RtmpGetStreamLengthCommand` data structure.
/// `RtmpGetStreamLengthCommand` 数据结构.
#[derive(Debug, Clone)]
pub struct RtmpGetStreamLengthCommand {
    /// `transaction_id` field of type `TransactionId`.
    /// `transaction_id` 字段，类型为 `TransactionId`.
    pub transaction_id: TransactionId,
    /// `stream_name` field of type `String`.
    /// `stream_name` 字段，类型为 `String`.
    pub stream_name: String,
}

impl RtmpGetStreamLengthCommand {
    fn from_message(
        transaction_id: TransactionId,
        _object: AmfValue,
        args: Vec<AmfValue>,
    ) -> Result<Self, Error> {
        let stream_name = args
            .first()
            .ok_or_else(|| Error::invalid_data("getStreamLength: missing stream name"))?
            .expect_str()?
            .to_string();
        Ok(Self {
            transaction_id,
            stream_name,
        })
    }
}

/// `RtmpResultCommand` data structure.
/// `RtmpResultCommand` 数据结构.
#[derive(Debug, Clone)]
pub struct RtmpResultCommand {
    /// `transaction_id` field of type `TransactionId`.
    /// `transaction_id` 字段，类型为 `TransactionId`.
    pub transaction_id: TransactionId,
    /// `properties` field of type `AmfValue`.
    /// `properties` 字段，类型为 `AmfValue`.
    pub properties: AmfValue,
    /// `information` field of type `AmfValue`.
    /// `information` 字段，类型为 `AmfValue`.
    pub information: AmfValue,
}

impl RtmpResultCommand {
    /// Returns the `stream_length_result` value.
    /// 返回 `stream_length_result` 值.
    pub fn get_stream_length_result(transaction_id: TransactionId, length: f64) -> Self {
        Self {
            transaction_id,
            properties: AmfValue::Amf0(Amf0Value::Null),
            information: AmfValue::Amf0(Amf0Value::Number(length)),
        }
    }

    fn from_message(
        transaction_id: TransactionId,
        object: AmfValue,
        args: Vec<AmfValue>,
    ) -> Result<Self, Error> {
        let properties = object;
        let information = args
            .first()
            .cloned()
            .ok_or_else(|| Error::invalid_data("_result: missing information argument"))?;
        Ok(Self {
            transaction_id,
            properties,
            information,
        })
    }

    /// Returns `true` if `error` is true.
    /// 返回 `真` 如果 `error` is 真.
    pub fn is_error(&self) -> bool {
        self.information
            .expect_object_member("code")
            .and_then(|code| code.expect_str())
            .map(|code| code.to_lowercase().contains("error"))
            .unwrap_or(false)
    }

    /// `create_stream_result` function.
    /// `create_stream_result` 函数.
    pub fn create_stream_result(
        transaction_id: TransactionId,
        stream_id: RtmpMessageStreamId,
    ) -> Self {
        Self {
            transaction_id,
            properties: AmfValue::Amf0(Amf0Value::Null),
            information: AmfValue::Amf0(Amf0Value::Number(stream_id.get() as f64)),
        }
    }
}

/// `RtmpOnStatusCommand` data structure.
/// `RtmpOnStatusCommand` 数据结构.
#[derive(Debug, Clone)]
pub struct RtmpOnStatusCommand {
    /// `level` field of type `String`.
    /// `level` 字段，类型为 `String`.
    pub level: String,
    /// `code` field of type `String`.
    /// `code` 字段，类型为 `String`.
    pub code: String,
    /// `description` field.
    /// `description` 字段.
    pub description: Option<String>,
    /// `details` field.
    /// `details` 字段.
    pub details: Option<String>,
}

impl RtmpOnStatusCommand {
    fn from_message(
        _transaction_id: TransactionId,
        _object: AmfValue,
        args: Vec<AmfValue>,
    ) -> Result<Self, Error> {
        let status_obj = args
            .first()
            .ok_or_else(|| Error::invalid_data("onStatus: missing status object"))?;

        let level = status_obj
            .expect_object_member("level")?
            .expect_str()?
            .to_string();
        let code = status_obj
            .expect_object_member("code")?
            .expect_str()?
            .to_string();

        let description = status_obj
            .expect_object_member("description")
            .ok()
            .and_then(|v| v.expect_str().ok())
            .map(|s| s.to_string());

        let details = status_obj
            .expect_object_member("details")
            .ok()
            .and_then(|v| v.expect_str().ok())
            .map(|s| s.to_string());

        Ok(Self {
            level,
            code,
            description,
            details,
        })
    }

    /// Returns `true` if `publish_start` is true.
    /// 返回 `真` 如果 `publish_start` is 真.
    pub fn is_publish_start(&self) -> bool {
        self.code == "NetStream.Publish.Start"
    }

    /// Returns `true` if `play_start` is true.
    /// 返回 `真` 如果 `play_start` is 真.
    pub fn is_play_start(&self) -> bool {
        self.code == "NetStream.Play.Start"
    }

    /// `publish_start` function.
    /// `publish_start` 函数.
    pub fn publish_start() -> Self {
        Self {
            level: "status".to_string(),
            code: "NetStream.Publish.Start".to_string(),
            description: Some("Publish succeeded.".to_string()),
            details: None,
        }
    }

    /// `play_start` function.
    /// `play_start` 函数.
    pub fn play_start() -> Self {
        Self {
            level: "status".to_string(),
            code: "NetStream.Play.Start".to_string(),
            description: Some("Play succeeded.".to_string()),
            details: None,
        }
    }

    /// `publish_bad_name` function.
    /// `publish_bad_name` 函数.
    pub fn publish_bad_name(reason: &str) -> Self {
        Self {
            level: "error".to_string(),
            code: "NetStream.Publish.BadName".to_string(),
            description: Some("Stream name already in use.".to_string()),
            details: Some(reason.to_string()),
        }
    }

    /// `play_stream_not_found` function.
    /// `play_stream_not_found` 函数.
    pub fn play_stream_not_found(reason: &str) -> Self {
        Self {
            level: "error".to_string(),
            code: "NetStream.Play.StreamNotFound".to_string(),
            description: Some("Stream not found.".to_string()),
            details: Some(reason.to_string()),
        }
    }
}
