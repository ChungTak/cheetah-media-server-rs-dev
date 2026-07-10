use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::amf::{AmfValue, AmfVersion};
use crate::amf0::Amf0Value as WireAmf0Value;
use crate::chunk::RtmpChunkSize;
use crate::command::{
    RtmpCommand as WireCommand, RtmpConnectCommand, RtmpCreateStreamCommand, RtmpOnStatusCommand,
    RtmpPlayCommand, RtmpPublishCommand, RtmpResultCommand, TransactionId,
};
use crate::message::{RtmpMessage, RtmpMessageHeader, RtmpMessageStreamId};
use crate::timestamp::RtmpTimestamp;
use crate::user_control::RtmpUserControlEvent;

use super::super::{
    ClientPendingAction, CoreOutput, HandshakeState, RtmpClientState, RtmpCore, RtmpCoreCommand,
    RtmpCoreError, RtmpEvent,
};

impl RtmpCore {
    /// `on_command` function.
    /// `on_command` 函数.
    pub(crate) fn on_command(
        &mut self,
        cmd: RtmpCoreCommand,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        match cmd {
            RtmpCoreCommand::SetWindowAckSize { size } => {
                self.send_rtmp_message(2, RtmpMessage::win_ack_size(size), out);
            }
            RtmpCoreCommand::SetPeerBandwidth { size } => {
                self.send_rtmp_message(2, RtmpMessage::set_peer_bandwidth(size), out);
            }
            RtmpCoreCommand::SetChunkSize { size } => {
                self.send_set_chunk_size(size, out)?;
            }
            RtmpCoreCommand::SendAck { sequence_number } => {
                self.send_rtmp_message(2, RtmpMessage::ack(sequence_number), out);
            }
            RtmpCoreCommand::SendPingResponse { timestamp } => {
                self.send_rtmp_message(
                    2,
                    RtmpMessage::UserControl {
                        header: RtmpMessageHeader::PCM,
                        event: RtmpUserControlEvent::PingResponse { timestamp },
                    },
                    out,
                );
            }
            RtmpCoreCommand::ClientConnect {
                app,
                flash_ver,
                tc_url,
            } => {
                self.send_connect_request(app, flash_ver, tc_url, out)?;
            }
            RtmpCoreCommand::ClientCreateStream { transaction_id } => {
                let transaction_id = TransactionId::from_f64(transaction_id);
                self.client_create_stream_transaction_id = Some(transaction_id.get());
                self.send_create_stream_request(transaction_id, out)?;
            }
            RtmpCoreCommand::ClientPublish {
                stream_id,
                transaction_id,
                stream_name,
            } => {
                self.client_pending_action = Some(ClientPendingAction::Publish);
                self.send_publish_request(stream_id, transaction_id, stream_name, out)?;
            }
            RtmpCoreCommand::ClientPlay {
                stream_id,
                transaction_id,
                stream_name,
                start,
            } => {
                self.client_pending_action = Some(ClientPendingAction::Play);
                self.send_play_request(stream_id, transaction_id, stream_name, start, out)?;
            }
            RtmpCoreCommand::ClientSeek { stream_id, millis } => {
                self.send_seek_request(stream_id, millis, out)?;
            }
            RtmpCoreCommand::ClientPause {
                stream_id,
                pause,
                millis,
            } => {
                self.send_pause_request(stream_id, pause, millis, out)?;
            }
            RtmpCoreCommand::ClientHandleWireCommand {
                message_stream_id: _message_stream_id,
                name,
                transaction_id,
                object,
                args,
            } => match WireCommand::from_message(&name, transaction_id, object, args) {
                Ok(WireCommand::Result(result)) => self.emit_result_received(result, out),
                Ok(WireCommand::OnStatus(status)) => self.emit_on_status_received(status, out),
                Ok(command) => {
                    out.push(CoreOutput::Event(RtmpEvent::CommandIgnored {
                        name,
                        detail: format!("client ignored command: {command:?}"),
                    }));
                }
                Err(error) => {
                    out.push(CoreOutput::Event(RtmpEvent::CommandIgnored {
                        name,
                        detail: format!("client command decode failed: {error}"),
                    }));
                }
            },
            RtmpCoreCommand::ClientObserveAck { sequence_number } => {
                out.push(CoreOutput::Event(RtmpEvent::AckReceived {
                    sequence_number,
                }));
            }
            RtmpCoreCommand::ClientObserveWinAckSize { size } => {
                self.peer_ack_window_size = size as u64;
                out.push(CoreOutput::Event(RtmpEvent::PeerAckWindowUpdated { size }));
            }
            RtmpCoreCommand::ClientHandleSetPeerBandwidth {
                size,
                response_window_size,
            } => {
                self.send_rtmp_message(2, RtmpMessage::win_ack_size(response_window_size), out);
                out.push(CoreOutput::Event(RtmpEvent::LocalAckWindowUpdated { size }));
            }
            RtmpCoreCommand::ClientObserveMediaData {
                stream_id,
                timestamp_ms,
                media_type,
                payload,
            } => {
                out.push(CoreOutput::Event(RtmpEvent::MediaData {
                    stream_id,
                    timestamp_ms,
                    media_type,
                    payload,
                }));
            }
            RtmpCoreCommand::ClientHandleUserControl { event } => match event {
                RtmpUserControlEvent::PingRequest { timestamp } => {
                    self.send_rtmp_message(
                        2,
                        RtmpMessage::UserControl {
                            header: RtmpMessageHeader::PCM,
                            event: RtmpUserControlEvent::PingResponse { timestamp },
                        },
                        out,
                    );
                }
                event => {
                    out.push(CoreOutput::Event(RtmpEvent::UserControlIgnored {
                        name: event.name().to_string(),
                        detail: format!("{event:?}"),
                    }));
                }
            },
            RtmpCoreCommand::ClientHandleUnhandledMessage { message } => {
                out.push(CoreOutput::Event(RtmpEvent::MessageIgnored {
                    name: format!("{:?}", message.message_type()),
                    detail: format!("{message:?}"),
                }));
            }
            RtmpCoreCommand::AcceptPublish { stream_id } => {
                self.active_publish = Some(stream_id);
                self.flush_pending_publish_media(stream_id, out);
                self.send_on_status(
                    stream_id,
                    "status",
                    "NetStream.Publish.Start",
                    "Start publishing.",
                    out,
                )?;
            }
            RtmpCoreCommand::RejectPublish {
                stream_id,
                description,
            } => {
                if self.pending_publish == Some(stream_id) {
                    self.clear_pending_publish();
                }
                self.send_on_status(
                    stream_id,
                    "error",
                    "NetStream.Publish.BadName",
                    &description,
                    out,
                )?;
            }
            RtmpCoreCommand::AcceptPlay { stream_id } => {
                self.send_accept_play(stream_id, true, true, out)?;
            }
            RtmpCoreCommand::AcceptPlayConfigured {
                stream_id,
                emit_play_status,
                emit_sample_access,
            } => {
                self.send_accept_play(stream_id, emit_play_status, emit_sample_access, out)?;
            }
            RtmpCoreCommand::RejectPlay {
                stream_id,
                description,
            } => {
                self.send_on_status(
                    stream_id,
                    "error",
                    "NetStream.Play.StreamNotFound",
                    &description,
                    out,
                )?;
            }
            RtmpCoreCommand::SendMetadata {
                stream_id,
                timestamp_ms,
                payload,
            } => {
                self.send_message(6, timestamp_ms, 18, stream_id, payload, out)?;
            }
            RtmpCoreCommand::SendAudio {
                stream_id,
                timestamp_ms,
                payload,
            } => {
                self.send_message(4, timestamp_ms, 8, stream_id, payload, out)?;
            }
            RtmpCoreCommand::SendVideo {
                stream_id,
                timestamp_ms,
                payload,
            } => {
                self.send_message(6, timestamp_ms, 9, stream_id, payload, out)?;
            }
            RtmpCoreCommand::SendNotify {
                stream_id,
                timestamp_ms,
                payload,
            } => {
                self.send_message(6, timestamp_ms, 18, stream_id, payload, out)?;
            }
            RtmpCoreCommand::CloseStream { stream_id } => {
                self.send_user_control(1, stream_id, out)?;
            }
            RtmpCoreCommand::CloseConnection => {
                self.state = HandshakeState::Closed;
                self.active_publish = None;
                self.clear_pending_publish();
                self.client_create_stream_transaction_id = None;
                self.client_pending_action = None;
                out.push(CoreOutput::Event(RtmpEvent::PeerClosed));
            }
        }

        Ok(())
    }

    /// `emit_result_received` function.
    /// `emit_result_received` 函数.
    pub(crate) fn emit_result_received(
        &mut self,
        result: RtmpResultCommand,
        out: &mut Vec<CoreOutput>,
    ) {
        let transaction_id = result.transaction_id.get();
        let is_error = result.is_error();
        let description = result
            .properties
            .expect_object_member("description")
            .ok()
            .and_then(|desc| desc.expect_str().ok())
            .map(|s| s.to_string())
            .or_else(|| {
                result
                    .information
                    .expect_object_member("description")
                    .ok()
                    .and_then(|desc| desc.expect_str().ok())
                    .map(|s| s.to_string())
            });

        if is_error {
            self.client_create_stream_transaction_id = None;
            self.client_pending_action = None;
            out.push(CoreOutput::Event(RtmpEvent::ClientDisconnectRequested {
                reason: format!(
                    "Command response error: {}",
                    description.unwrap_or_else(|| "Unknown error".to_string())
                ),
            }));
            return;
        }

        if transaction_id == TransactionId::CONNECT.get() {
            out.push(CoreOutput::Event(RtmpEvent::ClientStateChanged {
                state: RtmpClientState::Connected,
            }));
            return;
        }

        if self.client_create_stream_transaction_id == Some(transaction_id) {
            self.client_create_stream_transaction_id = None;
            out.push(CoreOutput::Event(RtmpEvent::ClientStateChanged {
                state: RtmpClientState::MediaStreamCreated,
            }));
            return;
        }

        out.push(CoreOutput::Event(RtmpEvent::CommandIgnored {
            name: "_result".to_string(),
            detail: format!("unhandled transaction id: {transaction_id}"),
        }));
    }

    /// `emit_on_status_received` function.
    /// `emit_on_status_received` 函数.
    pub(crate) fn emit_on_status_received(
        &mut self,
        status: RtmpOnStatusCommand,
        out: &mut Vec<CoreOutput>,
    ) {
        let RtmpOnStatusCommand {
            level,
            code,
            description,
            details,
        } = status;

        if code == "NetStream.Publish.Start"
            && self.client_pending_action == Some(ClientPendingAction::Publish)
        {
            self.client_pending_action = None;
            out.push(CoreOutput::Event(RtmpEvent::ClientStateChanged {
                state: RtmpClientState::Publishing,
            }));
            return;
        }

        if code == "NetStream.Play.Start"
            && self.client_pending_action == Some(ClientPendingAction::Play)
        {
            self.client_pending_action = None;
            out.push(CoreOutput::Event(RtmpEvent::ClientStateChanged {
                state: RtmpClientState::Playing,
            }));
            return;
        }

        if level == "error" {
            self.client_pending_action = None;
            let mut reason = format!("OnStatus error: {code}");
            if let Some(desc) = description {
                reason.push_str(&format!(" - {desc}"));
            }
            if let Some(detail) = details {
                reason.push_str(&format!(" ({detail})"));
            }
            out.push(CoreOutput::Event(RtmpEvent::ClientDisconnectRequested {
                reason,
            }));
            return;
        }

        out.push(CoreOutput::Event(RtmpEvent::CommandIgnored {
            name: "onStatus".to_string(),
            detail: format!("level={level}, code={code}"),
        }));
    }

    fn send_connect_request(
        &mut self,
        app: String,
        flash_ver: String,
        tc_url: String,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let command = WireCommand::Connect(RtmpConnectCommand {
            app,
            flash_ver,
            tc_url,
        });
        let message = command
            .into_message(RtmpMessageHeader::PCM)
            .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        self.send_rtmp_message(3, message, out);
        Ok(())
    }

    fn send_create_stream_request(
        &mut self,
        transaction_id: TransactionId,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let command = WireCommand::CreateStream(RtmpCreateStreamCommand { transaction_id });
        let message = command
            .into_message(RtmpMessageHeader::PCM)
            .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        self.send_rtmp_message(3, message, out);
        Ok(())
    }

    fn send_publish_request(
        &mut self,
        stream_id: u32,
        transaction_id: f64,
        stream_name: String,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let command = WireCommand::Publish(RtmpPublishCommand {
            transaction_id: TransactionId::from_f64(transaction_id),
            stream_name,
        });
        let header = RtmpMessageHeader {
            stream_id: RtmpMessageStreamId::new(stream_id),
            timestamp: RtmpTimestamp::ZERO,
        };
        let message = command
            .into_message(header)
            .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        self.send_rtmp_message(8, message, out);
        Ok(())
    }

    fn send_play_request(
        &mut self,
        stream_id: u32,
        transaction_id: f64,
        stream_name: String,
        start: f64,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let command = WireCommand::Play(RtmpPlayCommand {
            transaction_id: TransactionId::from_f64(transaction_id),
            stream_name,
            start,
        });
        let header = RtmpMessageHeader {
            stream_id: RtmpMessageStreamId::new(stream_id),
            timestamp: RtmpTimestamp::ZERO,
        };
        let message = command
            .into_message(header)
            .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        self.send_rtmp_message(8, message, out);
        Ok(())
    }

    /// `send_window_ack_size` function.
    /// `send_window_ack_size` 函数.
    pub(crate) fn send_window_ack_size(
        &mut self,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        self.send_rtmp_message(2, RtmpMessage::win_ack_size(5_000_000), out);
        Ok(())
    }

    /// `send_set_peer_bandwidth` function.
    /// `send_set_peer_bandwidth` 函数.
    pub(crate) fn send_set_peer_bandwidth(
        &mut self,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        self.send_rtmp_message(2, RtmpMessage::set_peer_bandwidth(5_000_000), out);
        Ok(())
    }

    /// `send_set_chunk_size` function.
    /// `send_set_chunk_size` 函数.
    pub(crate) fn send_set_chunk_size(
        &mut self,
        chunk_size: u32,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let chunk_size = RtmpChunkSize::saturating_new(chunk_size as usize);
        self.out_chunk_size = chunk_size.get();
        self.send_rtmp_message(2, RtmpMessage::set_chunk_size(chunk_size), out);
        Ok(())
    }

    /// `send_connect_result` function.
    /// `send_connect_result` 函数.
    pub(crate) fn send_connect_result(
        &mut self,
        txn: f64,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let cmd = WireCommand::Result(RtmpResultCommand {
            transaction_id: TransactionId::from_f64(txn),
            properties: AmfValue::amf0_object([
                ("fmsVer", WireAmf0Value::String("FMS/4,5,0,297".to_string())),
                ("capabilities", WireAmf0Value::Number(255.0)),
                ("mode", WireAmf0Value::Number(1.0)),
            ]),
            information: AmfValue::amf0_object([
                ("level", WireAmf0Value::String("status".to_string())),
                (
                    "code",
                    WireAmf0Value::String("NetConnection.Connect.Success".to_string()),
                ),
                (
                    "description",
                    WireAmf0Value::String("Connection succeeded.".to_string()),
                ),
                ("objectEncoding", WireAmf0Value::Number(0.0)),
            ]),
        });
        let message = cmd
            .into_message(RtmpMessageHeader::PCM)
            .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        self.send_rtmp_message(3, message, out);
        Ok(())
    }

    /// `send_create_stream_result` function.
    /// `send_create_stream_result` 函数.
    pub(crate) fn send_create_stream_result(
        &mut self,
        txn: f64,
        stream_id: u32,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let cmd = WireCommand::Result(RtmpResultCommand::create_stream_result(
            TransactionId::from_f64(txn),
            RtmpMessageStreamId::new(stream_id),
        ));
        let message = cmd
            .into_message(RtmpMessageHeader::PCM)
            .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        self.send_rtmp_message(3, message, out);
        Ok(())
    }

    /// `allocate_stream_id` function.
    /// `allocate_stream_id` 函数.
    pub(super) fn allocate_stream_id(&mut self) -> u32 {
        let current = self.next_stream_id.max(1);
        self.next_stream_id = self.next_stream_id.checked_add(1).unwrap_or(1);
        current
    }

    fn send_on_status(
        &mut self,
        stream_id: u32,
        level: &str,
        code: &str,
        description: &str,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let cmd = WireCommand::OnStatus(RtmpOnStatusCommand {
            level: level.to_string(),
            code: code.to_string(),
            description: Some(description.to_string()),
            details: None,
        });
        let header = RtmpMessageHeader {
            stream_id: RtmpMessageStreamId::new(stream_id),
            timestamp: RtmpTimestamp::ZERO,
        };
        let message = cmd
            .into_message(header)
            .map_err(|e| RtmpCoreError::Amf0(e.to_string()))?;
        self.send_rtmp_message(5, message, out);
        Ok(())
    }

    fn send_rtmp_sample_access(
        &mut self,
        stream_id: u32,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let message = RtmpMessage::Data {
            header: RtmpMessageHeader {
                stream_id: RtmpMessageStreamId::new(stream_id),
                timestamp: RtmpTimestamp::ZERO,
            },
            amf_version: AmfVersion::Amf0,
            values: vec![
                AmfValue::Amf0(WireAmf0Value::String("|RtmpSampleAccess".to_string())),
                AmfValue::Amf0(WireAmf0Value::Boolean(true)),
                AmfValue::Amf0(WireAmf0Value::Boolean(true)),
            ],
        };
        self.send_rtmp_message(6, message, out);
        Ok(())
    }

    fn send_accept_play(
        &mut self,
        stream_id: u32,
        emit_play_status: bool,
        emit_sample_access: bool,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        self.send_user_control(0, stream_id, out)?;

        if emit_play_status {
            self.send_on_status(
                stream_id,
                "status",
                "NetStream.Play.Reset",
                "Resetting and playing stream.",
                out,
            )?;
            self.send_on_status(
                stream_id,
                "status",
                "NetStream.Play.Start",
                "Started playing.",
                out,
            )?;
        }

        if emit_sample_access {
            self.send_rtmp_sample_access(stream_id, out)?;
        }
        Ok(())
    }

    /// Send a minimal `_result(txn, null)` response for side-band commands.
    pub(crate) fn send_seek_request(
        &mut self,
        stream_id: u32,
        millis: f64,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let payload = crate::amf0::encode_all(&[
            WireAmf0Value::String("seek".to_string()),
            WireAmf0Value::Number(0.0),
            WireAmf0Value::Null,
            WireAmf0Value::Number(millis),
        ]);
        self.send_message(8, 0, 20, stream_id, payload, out)
    }

    /// `send_pause_request` function.
    /// `send_pause_request` 函数.
    pub(crate) fn send_pause_request(
        &mut self,
        stream_id: u32,
        pause: bool,
        millis: f64,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let payload = crate::amf0::encode_all(&[
            WireAmf0Value::String("pause".to_string()),
            WireAmf0Value::Number(0.0),
            WireAmf0Value::Null,
            WireAmf0Value::Boolean(pause),
            WireAmf0Value::Number(millis),
        ]);
        self.send_message(8, 0, 20, stream_id, payload, out)
    }

    /// `send_null_result` function.
    /// `send_null_result` 函数.
    pub(crate) fn send_null_result(&mut self, txn: f64, out: &mut Vec<CoreOutput>) {
        let payload = crate::amf0::encode_all(&[
            WireAmf0Value::String("_result".to_string()),
            WireAmf0Value::Number(txn),
            WireAmf0Value::Null,
        ]);
        let _ = self.send_message(3, 0, 20, 0, payload, out);
    }

    /// Send `onBWDone` after connect to satisfy clients that probe bandwidth.
    pub(crate) fn send_on_bw_done(&mut self, out: &mut Vec<CoreOutput>) {
        let payload = crate::amf0::encode_all(&[
            WireAmf0Value::String("onBWDone".to_string()),
            WireAmf0Value::Number(0.0),
            WireAmf0Value::Null,
        ]);
        let _ = self.send_message(3, 0, 20, 0, payload, out);
    }
}
