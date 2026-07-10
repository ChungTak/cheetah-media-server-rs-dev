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
/// RTMP transaction ID, stored as an integer even though it is sent as a float.
/// RTMP 事务 ID，虽然以浮点数发送，但内部用整数保存。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TransactionId(i64);

impl TransactionId {
    /// Transaction ID used for `onStatus` messages.
    /// `onStatus` 消息使用的事务 ID。
    pub const ON_STATUS: Self = Self(0);
    /// Transaction ID for the `connect` command.
    /// `connect` 命令的事务 ID。
    pub const CONNECT: Self = Self(1);
    /// Lowest transaction ID that is not reserved for built-in commands.
    /// 保留给内置命令之上的最小事务 ID。
    pub const NON_RESERVED_START: Self = Self(2);

    /// Rounds an `f64` transaction ID to the nearest integer.
    /// 将 f64 事务 ID 四舍五入为整数。
    pub fn from_f64(id: f64) -> Self {
        Self(round_f64_to_i64(id))
    }

    /// Returns the raw integer transaction ID.
    /// 返回原始整数事务 ID。
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Increments the transaction ID by one.
    /// 将事务 ID 加 1。
    pub fn increment(&mut self) {
        self.0 += 1
    }
}

/// Rounds `f64` to the nearest `i64` using half-away-from-zero.
/// 使用远离零的四舍五入将 f64 转为 i64。
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

/// A parsed RTMP command extracted from an AMF command message.
/// 从 AMF 命令消息中解析出的 RTMP 命令。
#[derive(Debug, Clone)]
pub enum RtmpCommand {
    /// `connect` command carrying application connection parameters.
    /// 携带应用连接参数的 `connect` 命令。
    Connect(RtmpConnectCommand),
    /// `createStream` command requesting a new stream ID.
    /// 请求新流 ID 的 `createStream` 命令。
    CreateStream(RtmpCreateStreamCommand),
    /// `publish` command to start publishing a stream.
    /// 开始发布流的 `publish` 命令。
    Publish(RtmpPublishCommand),
    /// `play` command to start playback of a stream.
    /// 开始播放流的 `play` 命令。
    Play(RtmpPlayCommand),
    /// `deleteStream` command to close a stream.
    /// 关闭流的 `deleteStream` 命令。
    DeleteStream(RtmpDeleteStreamCommand),
    /// `getStreamLength` command to query stream duration.
    /// 查询流时长的 `getStreamLength` 命令。
    GetStreamLength(RtmpGetStreamLengthCommand),
    /// `_result` or `_error` response command.
    /// `_result` 或 `_error` 响应命令。
    Result(RtmpResultCommand),
    /// `onStatus` status event command.
    /// `onStatus` 状态事件命令。
    OnStatus(RtmpOnStatusCommand),
    /// Command not handled by this crate, retained for debugging.
    /// 本 crate 未处理的命令，保留用于调试。
    Ignore {
        name: String,

        // 以下字段仅用于调试显示，不会在代码主体中引用
        object: AmfValue,
        args: Vec<AmfValue>,
    },
}

impl RtmpCommand {
    /// Returns the wire name of the command.
    /// 返回命令的线名称。
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

    /// Converts the command into an `RtmpMessage` with the given header.
    /// 将该命令转换为带有指定头部的 `RtmpMessage`。
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

    /// Converts the command into a protocol control stream message.
    /// 将该命令转换为协议控制流消息。
    pub fn into_pcm_message(self) -> Result<RtmpMessage, Error> {
        self.into_message(RtmpMessageHeader {
            stream_id: RtmpMessageStreamId::PCM,
            timestamp: RtmpTimestamp::ZERO,
        })
    }

    /// Parses a command from the decoded name, transaction ID, object, and arguments.
    /// 从已解码的名称、事务 ID、对象与参数中解析命令。
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

/// Parameters of an RTMP `connect` command.
/// RTMP `connect` 命令的参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpConnectCommand {
    pub app: String,
    pub flash_ver: String,
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

    /// Builds a successful `connect` `_result` message for the standard application.
    /// 为标准应用构建成功的 `connect` `_result` 消息。
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

/// A `createStream` command carrying the transaction ID.
/// 携带事务 ID 的 `createStream` 命令。
#[derive(Debug, Clone)]
pub struct RtmpCreateStreamCommand {
    pub transaction_id: TransactionId,
}

impl RtmpCreateStreamCommand {
    fn from_message(transaction_id: TransactionId, _object: AmfValue) -> Result<Self, Error> {
        Ok(Self { transaction_id })
    }
}

/// Parameters of a `publish` command.
/// `publish` 命令的参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpPublishCommand {
    pub transaction_id: TransactionId,
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

    /// Builds a successful `publish` `_result` message for the given stream.
    /// 为指定流构建成功的 `publish` `_result` 消息。
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

/// Parameters of a `play` command, including the start position.
/// `play` 命令的参数，包括起始位置。
#[derive(Debug, Clone)]
pub struct RtmpPlayCommand {
    pub transaction_id: TransactionId,
    pub stream_name: String,
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

    /// Builds a successful `play` `_result` message for the given stream.
    /// 为指定流构建成功的 `play` `_result` 消息。
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

/// Parameters of a `deleteStream` command.
/// `deleteStream` 命令的参数。
#[derive(Debug, Clone)]
pub struct RtmpDeleteStreamCommand {
    pub transaction_id: TransactionId,
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

/// Parameters of a `getStreamLength` command.
/// `getStreamLength` 命令的参数。
#[derive(Debug, Clone)]
pub struct RtmpGetStreamLengthCommand {
    pub transaction_id: TransactionId,
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

/// A parsed `_result` or `_error` command.
/// 解析后的 `_result` 或 `_error` 命令。
#[derive(Debug, Clone)]
pub struct RtmpResultCommand {
    pub transaction_id: TransactionId,
    pub properties: AmfValue,
    pub information: AmfValue,
}

impl RtmpResultCommand {
    /// Builds a `_result` for `getStreamLength` carrying the stream length.
    /// 为 `getStreamLength` 构建携带流长度的 `_result`。
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

    /// Returns true if the result code contains "error".
    /// 若结果码包含 "error" 则返回 true。
    pub fn is_error(&self) -> bool {
        self.information
            .expect_object_member("code")
            .and_then(|code| code.expect_str())
            .map(|code| code.to_lowercase().contains("error"))
            .unwrap_or(false)
    }

    /// Builds a `_result` for `createStream` returning the assigned stream ID.
    /// 为 `createStream` 构建返回已分配流 ID 的 `_result`。
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

/// A parsed `onStatus` event with level, code, and optional description.
/// 解析后的 `onStatus` 事件，包含级别、代码与可选描述。
#[derive(Debug, Clone)]
pub struct RtmpOnStatusCommand {
    pub level: String,
    pub code: String,
    pub description: Option<String>,
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

    /// Returns true if the status code indicates `NetStream.Publish.Start`.
    /// 若状态码为 `NetStream.Publish.Start` 则返回 true。
    pub fn is_publish_start(&self) -> bool {
        self.code == "NetStream.Publish.Start"
    }

    /// Returns true if the status code indicates `NetStream.Play.Start`.
    /// 若状态码为 `NetStream.Play.Start` 则返回 true。
    pub fn is_play_start(&self) -> bool {
        self.code == "NetStream.Play.Start"
    }

    /// Builds a successful `NetStream.Publish.Start` status.
    /// 构建成功的 `NetStream.Publish.Start` 状态。
    pub fn publish_start() -> Self {
        Self {
            level: "status".to_string(),
            code: "NetStream.Publish.Start".to_string(),
            description: Some("Publish succeeded.".to_string()),
            details: None,
        }
    }

    /// Builds a successful `NetStream.Play.Start` status.
    /// 构建成功的 `NetStream.Play.Start` 状态。
    pub fn play_start() -> Self {
        Self {
            level: "status".to_string(),
            code: "NetStream.Play.Start".to_string(),
            description: Some("Play succeeded.".to_string()),
            details: None,
        }
    }

    /// Builds a `NetStream.Publish.BadName` error status.
    /// 构建 `NetStream.Publish.BadName` 错误状态。
    pub fn publish_bad_name(reason: &str) -> Self {
        Self {
            level: "error".to_string(),
            code: "NetStream.Publish.BadName".to_string(),
            description: Some("Stream name already in use.".to_string()),
            details: Some(reason.to_string()),
        }
    }

    /// Builds a `NetStream.Play.StreamNotFound` error status.
    /// 构建 `NetStream.Play.StreamNotFound` 错误状态。
    pub fn play_stream_not_found(reason: &str) -> Self {
        Self {
            level: "error".to_string(),
            code: "NetStream.Play.StreamNotFound".to_string(),
            description: Some("Stream not found.".to_string()),
            details: Some(reason.to_string()),
        }
    }
}
