use alloc::string::{String, ToString};
use alloc::vec::Vec;

use bytes::Bytes;

use crate::amf::{AmfValue, AmfVersion};
use crate::amf0::Amf0Value as WireAmf0Value;
use crate::amf3::Amf3Value;
use crate::command::{RtmpCommand as WireCommand, RtmpResultCommand, TransactionId};
use crate::message::{RtmpMessageHeader, RtmpMessageStreamId};

use super::super::{CoreOutput, RtmpCore, RtmpCoreError, RtmpEvent};

impl RtmpCore {
    /// `on_notify_message` function.
    /// `on_notify_message` 函数.
    pub(crate) fn on_notify_message(
        &mut self,
        message_stream_id: u32,
        payload: Bytes,
    ) -> Result<Option<RtmpEvent>, RtmpCoreError> {
        let values = decode_amf_values(&payload, AmfVersion::Amf0)?;
        self.emit_notify_event(message_stream_id, values)
    }

    /// `on_notify_message_amf3` function.
    /// `on_notify_message_amf3` 函数.
    pub(crate) fn on_notify_message_amf3(
        &mut self,
        message_stream_id: u32,
        payload: Bytes,
    ) -> Result<Option<RtmpEvent>, RtmpCoreError> {
        let values = decode_amf_values(&payload, AmfVersion::Amf3)?;
        self.emit_notify_event(message_stream_id, values)
    }

    fn emit_notify_event(
        &self,
        message_stream_id: u32,
        values: Vec<AmfValue>,
    ) -> Result<Option<RtmpEvent>, RtmpCoreError> {
        if values.is_empty() {
            return Ok(None);
        }

        let first = values.first().and_then(amf_string).unwrap_or_default();
        if first.eq_ignore_ascii_case("@setDataFrame") {
            if values
                .get(1)
                .and_then(amf_string)
                .is_some_and(|v| v.eq_ignore_ascii_case("onMetaData"))
            {
                return Ok(Some(RtmpEvent::Metadata {
                    stream_id: message_stream_id,
                    values,
                }));
            }
        } else if first.eq_ignore_ascii_case("onMetaData") {
            return Ok(Some(RtmpEvent::Metadata {
                stream_id: message_stream_id,
                values,
            }));
        }

        Ok(Some(RtmpEvent::Notify {
            stream_id: message_stream_id,
            name: first.to_string(),
            values,
        }))
    }

    /// `on_command_message` function.
    /// `on_command_message` 函数.
    pub(crate) fn on_command_message(
        &mut self,
        message_stream_id: u32,
        payload: Bytes,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        self.on_command_message_with_version(message_stream_id, payload, AmfVersion::Amf0, out)
    }

    /// `on_command_message_amf3` function.
    /// `on_command_message_amf3` 函数.
    pub(crate) fn on_command_message_amf3(
        &mut self,
        message_stream_id: u32,
        payload: Bytes,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        self.on_command_message_with_version(message_stream_id, payload, AmfVersion::Amf3, out)
    }

    fn on_command_message_with_version(
        &mut self,
        message_stream_id: u32,
        payload: Bytes,
        amf_version: AmfVersion,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let Some(command) = decode_command(&payload, amf_version)? else {
            return Ok(());
        };

        let name_lower = command.name.to_ascii_lowercase();
        let parsed = canonical_command_name(&name_lower).and_then(|name| {
            WireCommand::from_message(
                name,
                command.transaction_id,
                command.object.clone(),
                command.args.clone(),
            )
            .ok()
        });

        match parsed {
            Some(WireCommand::Connect(connect)) => {
                self.handle_connect_command(
                    connect.app,
                    connect.tc_url,
                    command.transaction_id_raw,
                    out,
                )?;
            }
            Some(WireCommand::CreateStream(create_stream)) => {
                self.handle_create_stream_command(create_stream.transaction_id.get() as f64, out)?;
            }
            Some(WireCommand::GetStreamLength(get_stream_length)) => {
                self.handle_get_stream_length_command(get_stream_length.transaction_id, out)?;
            }
            Some(WireCommand::Publish(publish)) => {
                self.handle_publish_command(message_stream_id, publish.stream_name, out);
            }
            Some(WireCommand::Play(play)) => {
                self.handle_play_command(message_stream_id, play.stream_name, out);
            }
            Some(WireCommand::DeleteStream(delete_stream)) => {
                let target_stream_id = if delete_stream.stream_id.get() == 0 {
                    message_stream_id
                } else {
                    delete_stream.stream_id.get()
                };
                self.handle_delete_stream_command(target_stream_id, out);
            }
            Some(WireCommand::Result(result)) => {
                self.emit_result_received(result, out);
            }
            Some(WireCommand::OnStatus(status)) => {
                self.emit_on_status_received(status, out);
            }
            _ => self.handle_unstructured_command(command, message_stream_id, out)?,
        }

        Ok(())
    }
}

fn decode_amf_values(
    payload: &[u8],
    amf_version: AmfVersion,
) -> Result<Vec<AmfValue>, RtmpCoreError> {
    let (payload, amf_version) = normalize_amf_payload(payload, amf_version);

    let mut buf = payload;
    let mut values: Vec<AmfValue> = Vec::new();

    while !buf.is_empty() {
        let (size, value) =
            AmfValue::decode(buf, amf_version).map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        buf = &buf[size..];
        values.push(value);
    }

    Ok(values)
}

struct DecodedCommand {
    name: String,
    transaction_id: TransactionId,
    transaction_id_raw: f64,
    object: AmfValue,
    args: Vec<AmfValue>,
}

fn decode_command(
    payload: &[u8],
    amf_version: AmfVersion,
) -> Result<Option<DecodedCommand>, RtmpCoreError> {
    if payload.is_empty() {
        return Ok(None);
    }

    let (mut buf, effective_amf_version) = normalize_amf_payload(payload, amf_version);

    let (size, name) = AmfValue::decode(buf, effective_amf_version)
        .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
    buf = &buf[size..];
    let name = amf_string(&name).unwrap_or_default().to_string();

    let mut transaction_id_raw = 0.0;
    if !buf.is_empty() {
        let (size, transaction_id) = AmfValue::decode(buf, effective_amf_version)
            .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        buf = &buf[size..];
        if let Some(value) = amf_number(&transaction_id) {
            transaction_id_raw = value;
        }
    }

    let object = if buf.is_empty() {
        AmfValue::Amf0(WireAmf0Value::Null)
    } else {
        let (size, object) = AmfValue::decode(buf, effective_amf_version)
            .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        buf = &buf[size..];
        object
    };

    let mut args = Vec::new();
    while !buf.is_empty() {
        let (size, value) = AmfValue::decode(buf, effective_amf_version)
            .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        args.push(value);
        buf = &buf[size..];
    }

    Ok(Some(DecodedCommand {
        name,
        transaction_id: TransactionId::from_f64(transaction_id_raw),
        transaction_id_raw,
        object,
        args,
    }))
}

fn normalize_amf_payload(payload: &[u8], amf_version: AmfVersion) -> (&[u8], AmfVersion) {
    if amf_version == AmfVersion::Amf3 && payload.first() == Some(&0) {
        // Keep parity with RTMP flex-command handling:
        // leading 0 means the following payload is AMF0.
        (&payload[1..], AmfVersion::Amf0)
    } else {
        (payload, amf_version)
    }
}

fn canonical_command_name(name_lower: &str) -> Option<&'static str> {
    match name_lower {
        "connect" => Some("connect"),
        "createstream" => Some("createStream"),
        "publish" => Some("publish"),
        "play" => Some("play"),
        "deletestream" => Some("deleteStream"),
        "getstreamlength" => Some("getStreamLength"),
        "_result" => Some("_result"),
        "_error" => Some("_error"),
        "onstatus" => Some("onStatus"),
        _ => None,
    }
}

fn amf_string_at(args: &[AmfValue], idx: usize) -> Option<&str> {
    amf_string(args.get(idx)?)
}

fn object_member_string<'a>(object: &'a AmfValue, key: &str) -> Option<&'a str> {
    match object {
        AmfValue::Amf0(WireAmf0Value::Object { entries, .. })
        | AmfValue::Amf0(WireAmf0Value::EcmaArray { entries }) => entries
            .iter()
            .rfind(|entry| entry.key == key)
            .and_then(|entry| amf_string_from_amf0(&entry.value)),
        AmfValue::Amf3(Amf3Value::Object { entries, .. }) => entries
            .iter()
            .rfind(|entry| entry.key == key)
            .and_then(|entry| amf_string_from_amf3(&entry.value)),
        _ => None,
    }
}

fn command_target_stream_id(args: &[AmfValue]) -> Option<u32> {
    let stream_id = amf_number(args.first()?)?;
    if !stream_id.is_finite() || stream_id <= 0.0 || stream_id > u32::MAX as f64 {
        return None;
    }
    Some(stream_id as u32)
}

fn amf_string(value: &AmfValue) -> Option<&str> {
    match value {
        AmfValue::Amf0(v) => amf_string_from_amf0(v),
        AmfValue::Amf3(v) => amf_string_from_amf3(v),
    }
}

fn amf_number(value: &AmfValue) -> Option<f64> {
    match value {
        AmfValue::Amf0(WireAmf0Value::Number(v)) => Some(*v),
        AmfValue::Amf3(Amf3Value::Integer(v)) => Some(*v as f64),
        AmfValue::Amf3(Amf3Value::Double(v)) => Some(*v),
        _ => None,
    }
}

fn amf_bool(value: &AmfValue) -> Option<bool> {
    match value {
        AmfValue::Amf0(WireAmf0Value::Boolean(v)) => Some(*v),
        AmfValue::Amf0(WireAmf0Value::Number(v)) => Some(*v != 0.0),
        AmfValue::Amf3(Amf3Value::Boolean(v)) => Some(*v),
        _ => None,
    }
}

fn amf_string_from_amf0(value: &WireAmf0Value) -> Option<&str> {
    match value {
        WireAmf0Value::String(v) => Some(v.as_str()),
        _ => None,
    }
}

fn amf_string_from_amf3(value: &Amf3Value) -> Option<&str> {
    match value {
        Amf3Value::String(v) => Some(v.as_str()),
        _ => None,
    }
}

impl RtmpCore {
    fn start_pending_publish(
        &mut self,
        message_stream_id: u32,
        stream_name: String,
        out: &mut Vec<CoreOutput>,
    ) {
        self.active_publish = None;
        self.pending_publish = Some(message_stream_id);
        self.pending_media.clear();
        self.pending_media_bytes = 0;

        let app = self
            .connected_app
            .clone()
            .unwrap_or_else(|| "live".to_string());
        let tc_url = self
            .connected_tc_url
            .clone()
            .unwrap_or_else(|| format!("rtmp:///{app}"));
        out.push(CoreOutput::Event(RtmpEvent::PublishRequested {
            stream_id: message_stream_id,
            app,
            tc_url,
            stream_name,
        }));
    }

    fn emit_play_requested(
        &mut self,
        message_stream_id: u32,
        stream_name: String,
        out: &mut Vec<CoreOutput>,
    ) {
        let app = self
            .connected_app
            .clone()
            .unwrap_or_else(|| "live".to_string());
        let tc_url = self
            .connected_tc_url
            .clone()
            .unwrap_or_else(|| format!("rtmp:///{app}"));
        out.push(CoreOutput::Event(RtmpEvent::PlayRequested {
            stream_id: message_stream_id,
            app,
            tc_url,
            stream_name,
        }));
    }

    fn close_stream(&mut self, stream_id: u32, out: &mut Vec<CoreOutput>) {
        if self.active_publish == Some(stream_id) {
            self.active_publish = None;
        }
        if self.pending_publish == Some(stream_id) {
            self.clear_pending_publish();
        }
        out.push(CoreOutput::Event(RtmpEvent::StreamClosed { stream_id }));
    }

    fn handle_connect_command(
        &mut self,
        app: String,
        tc_url: String,
        transaction_id_raw: f64,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        self.connected_app = Some(app.clone());
        self.connected_tc_url = Some(tc_url.clone());
        self.send_connect_result(transaction_id_raw, out)?;
        self.send_on_bw_done(out);
        out.push(CoreOutput::Event(RtmpEvent::Connected { app, tc_url }));
        Ok(())
    }

    fn handle_create_stream_command(
        &mut self,
        transaction_id_raw: f64,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let stream_id = self.allocate_stream_id();
        self.send_create_stream_result(transaction_id_raw, stream_id, out)?;
        out.push(CoreOutput::Event(RtmpEvent::StreamCreated { stream_id }));
        Ok(())
    }

    fn handle_get_stream_length_command(
        &mut self,
        transaction_id: TransactionId,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let message = WireCommand::Result(RtmpResultCommand::get_stream_length_result(
            transaction_id,
            0.0,
        ))
        .into_message(RtmpMessageHeader {
            stream_id: RtmpMessageStreamId::PCM,
            timestamp: crate::timestamp::RtmpTimestamp::ZERO,
        })
        .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        self.send_rtmp_message(3, message, out);
        Ok(())
    }

    fn handle_publish_command(
        &mut self,
        message_stream_id: u32,
        stream_name: String,
        out: &mut Vec<CoreOutput>,
    ) {
        self.start_pending_publish(message_stream_id, stream_name, out);
    }

    fn handle_play_command(
        &mut self,
        message_stream_id: u32,
        stream_name: String,
        out: &mut Vec<CoreOutput>,
    ) {
        self.emit_play_requested(message_stream_id, stream_name, out);
    }

    fn handle_delete_stream_command(&mut self, stream_id: u32, out: &mut Vec<CoreOutput>) {
        self.close_stream(stream_id, out);
    }

    fn handle_unstructured_command(
        &mut self,
        command: DecodedCommand,
        message_stream_id: u32,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let name_lower = command.name.to_ascii_lowercase();

        match name_lower.as_str() {
            "connect" => {
                let app = object_member_string(&command.object, "app")
                    .unwrap_or("live")
                    .to_string();
                let tc_url = object_member_string(&command.object, "tcUrl")
                    .unwrap_or("")
                    .to_string();
                self.handle_connect_command(app, tc_url, command.transaction_id_raw, out)?;
            }
            "createstream" => {
                self.handle_create_stream_command(command.transaction_id_raw, out)?;
            }
            "getstreamlength" => {
                self.handle_get_stream_length_command(command.transaction_id, out)?;
            }
            "publish" => {
                let stream_name = amf_string_at(&command.args, 0).unwrap_or("").to_string();
                self.handle_publish_command(message_stream_id, stream_name, out);
            }
            "play" | "play2" => {
                let stream_name = amf_string_at(&command.args, 0).unwrap_or("").to_string();
                self.handle_play_command(message_stream_id, stream_name, out);
            }
            "deletestream" | "closestream" => {
                let target_stream_id =
                    command_target_stream_id(&command.args).unwrap_or(message_stream_id);
                self.handle_delete_stream_command(target_stream_id, out);
            }
            // Side-band commands observed in common encoders/players.
            // Respond with _result to satisfy clients that wait for acknowledgement.
            "fcpublish" | "fcunpublish" | "releasestream" | "_checkbw" => {
                self.send_null_result(command.transaction_id_raw, out);
                out.push(CoreOutput::Event(RtmpEvent::CommandIgnored {
                    name: command.name,
                    detail: "side-band command acknowledged".to_string(),
                }));
            }
            "seek" => {
                let millis = command.args.first().and_then(amf_number).unwrap_or(0.0);
                self.send_null_result(command.transaction_id_raw, out);
                out.push(CoreOutput::Event(RtmpEvent::SeekRequested {
                    stream_id: message_stream_id,
                    millis,
                }));
            }
            "pause" => {
                let pause = command.args.first().and_then(amf_bool).unwrap_or(true);
                let millis = command.args.get(1).and_then(amf_number).unwrap_or(0.0);
                self.send_null_result(command.transaction_id_raw, out);
                out.push(CoreOutput::Event(RtmpEvent::PauseRequested {
                    stream_id: message_stream_id,
                    pause,
                    millis,
                }));
            }
            "receivevideo" => {
                let enabled = command.args.first().and_then(amf_bool).unwrap_or(true);
                out.push(CoreOutput::Event(RtmpEvent::ReceiveVideo {
                    stream_id: message_stream_id,
                    enabled,
                }));
            }
            "receiveaudio" => {
                let enabled = command.args.first().and_then(amf_bool).unwrap_or(true);
                out.push(CoreOutput::Event(RtmpEvent::ReceiveAudio {
                    stream_id: message_stream_id,
                    enabled,
                }));
            }
            _ => {
                out.push(CoreOutput::Event(RtmpEvent::CommandIgnored {
                    name: command.name,
                    detail: "unsupported command ignored".to_string(),
                }));
            }
        }
        Ok(())
    }
}
