use std::collections::HashMap;
use std::net::SocketAddr;

use crate::error::Gb28181Diagnostic;
use crate::message::{SipMessage, StartLine};

pub type GbDeviceId = String;
pub type GbSessionId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogState {
    Trying,
    Confirmed,
    Terminated,
}

#[derive(Debug, Clone)]
pub struct GbDevice {
    pub id: GbDeviceId,
    pub contact_addr: SocketAddr,
    pub expires_at_ms: u64,
    pub last_keepalive_ms: u64,
}

#[derive(Debug, Clone)]
pub struct GbInviteSpec {
    pub session_key: String,
    pub ssrc: u32,
    pub destination: SocketAddr,
    pub app_name: String,
    pub stream_name: String,
    pub is_video: bool,
    /// Local IP for the SDP `c=IN IP4 ...` line and the `m=` media address.
    pub local_ip: String,
    /// Local RTP port to advertise in the SDP `m=` line for receiving media.
    pub local_port: u16,
}

#[derive(Debug, Clone)]
pub struct GbTalkSpec {
    pub session_key: String,
    pub ssrc: u32,
    pub destination: SocketAddr,
    pub app_name: String,
    pub stream_name: String,
    /// Local IP for the SDP `c=IN IP4 ...` line.
    pub local_ip: String,
    /// Local RTP port to advertise for the talk session.
    pub local_port: u16,
}

#[derive(Debug, Clone)]
pub enum Gb28181Command {
    RegisterChallenge {
        device_id: GbDeviceId,
        destination: SocketAddr,
    },
    StartInvite(GbInviteSpec),
    StopInvite(GbSessionId),
    StartTalk(GbTalkSpec),
    StopTalk(GbSessionId),
}

#[derive(Debug, Clone)]
pub enum Gb28181Event {
    DeviceRegistered {
        device_id: GbDeviceId,
        contact_addr: SocketAddr,
    },
    DeviceKeepalive {
        device_id: GbDeviceId,
    },
    DeviceOffline {
        device_id: GbDeviceId,
    },
    InviteSuccess {
        session_key: String,
        ssrc: u32,
    },
    InviteClosed {
        session_key: String,
    },
}

#[derive(Debug, Clone)]
pub struct SipSendAction {
    pub destination: SocketAddr,
    pub message: SipMessage,
}

#[derive(Debug, Clone)]
pub enum Gb28181CoreInput {
    SipMessage {
        source: SocketAddr,
        message: SipMessage,
    },
    Tick {
        now_ms: u64,
    },
    Command(Gb28181Command),
}

#[derive(Debug, Clone)]
pub enum Gb28181CoreOutput {
    SendSip(SipSendAction),
    Event(Gb28181Event),
    Diagnostic(Gb28181Diagnostic),
}

struct GbInviteSession {
    session_key: String,
    ssrc: u32,
    destination: SocketAddr,
    call_id: String,
    state: DialogState,
    local_ip: String,
    local_port: u16,
    app_name: String,
    _last_activity_ms: u64,
}

pub struct Gb28181Core {
    devices: HashMap<GbDeviceId, GbDevice>,
    sessions: HashMap<GbSessionId, GbInviteSession>,
    next_call_id_seq: u64,
    /// Last known time from Tick input; 0 if no Tick received yet.
    now_ms: u64,
}

impl Default for Gb28181Core {
    fn default() -> Self {
        Self::new()
    }
}

impl Gb28181Core {
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
            sessions: HashMap::new(),
            next_call_id_seq: 1,
            now_ms: 0,
        }
    }

    pub fn handle_input(&mut self, input: Gb28181CoreInput, outputs: &mut Vec<Gb28181CoreOutput>) {
        match input {
            Gb28181CoreInput::SipMessage { source, message } => {
                let now_ms = self.now_ms;
                self.process_sip_message(source, message, now_ms, outputs);
            }
            Gb28181CoreInput::Tick { now_ms } => {
                self.now_ms = now_ms;
                self.check_timeouts(now_ms, outputs);
            }
            Gb28181CoreInput::Command(cmd) => {
                let now_ms = self.now_ms;
                self.process_command(cmd, now_ms, outputs);
            }
        }
    }

    fn process_sip_message(
        &mut self,
        source: SocketAddr,
        msg: SipMessage,
        now_ms: u64,
        outputs: &mut Vec<Gb28181CoreOutput>,
    ) {
        match &msg.start_line {
            StartLine::Request {
                method,
                uri,
                version,
            } => {
                match method.as_str() {
                    "REGISTER" => {
                        self.handle_register(source, uri, version, &msg, now_ms, outputs);
                    }
                    "MESSAGE" => {
                        self.handle_message(source, &msg, now_ms, outputs);
                    }
                    "BYE" => {
                        self.handle_bye(source, &msg, outputs);
                    }
                    _ => {
                        // Return 501 Not Implemented
                        let mut resp = SipMessage {
                            start_line: StartLine::Response {
                                version: "SIP/2.0".to_string(),
                                status: 501,
                                reason: "Not Implemented".to_string(),
                            },
                            headers: Vec::new(),
                            body: Vec::new(),
                        };
                        if let Some(via) = msg.get_header("Via") {
                            resp.set_header("Via", via);
                        }
                        if let Some(from) = msg.get_header("From") {
                            resp.set_header("From", from);
                        }
                        if let Some(to) = msg.get_header("To") {
                            resp.set_header("To", to);
                        }
                        if let Some(call_id) = msg.get_header("Call-ID") {
                            resp.set_header("Call-ID", call_id);
                        }
                        if let Some(cseq) = msg.get_header("CSeq") {
                            resp.set_header("CSeq", cseq);
                        }
                        resp.set_header("Content-Length", "0");
                        outputs.push(Gb28181CoreOutput::SendSip(SipSendAction {
                            destination: source,
                            message: resp,
                        }));
                    }
                }
            }
            StartLine::Response { status, reason, .. } => {
                self.handle_response(source, *status, reason, &msg, outputs);
            }
        }
    }

    fn handle_register(
        &mut self,
        source: SocketAddr,
        _uri: &str,
        _version: &str,
        msg: &SipMessage,
        now_ms: u64,
        outputs: &mut Vec<Gb28181CoreOutput>,
    ) {
        // Extract device id from From header
        // From: <sip:34020000001320000001@3402000000>;tag=...
        let from = msg.get_header("From").unwrap_or("");
        let device_id = extract_sip_username(from).unwrap_or_else(|| "unknown".to_string());

        if device_id == "unknown" {
            outputs.push(Gb28181CoreOutput::Diagnostic(
                Gb28181Diagnostic::SyntaxWarning {
                    raw: from.to_string(),
                    issue: "Missing or invalid From device ID".to_string(),
                },
            ));
            return;
        }

        // For simplicity, we accept registrations directly or support basic OK response
        let mut resp = SipMessage {
            start_line: StartLine::Response {
                version: "SIP/2.0".to_string(),
                status: 200,
                reason: "OK".to_string(),
            },
            headers: Vec::new(),
            body: Vec::new(),
        };

        if let Some(via) = msg.get_header("Via") {
            resp.set_header("Via", via);
        }
        if let Some(from_hdr) = msg.get_header("From") {
            resp.set_header("From", from_hdr);
        }
        if let Some(to_hdr) = msg.get_header("To") {
            resp.set_header("To", to_hdr);
        }
        if let Some(call_id) = msg.get_header("Call-ID") {
            resp.set_header("Call-ID", call_id);
        }
        if let Some(cseq) = msg.get_header("CSeq") {
            resp.set_header("CSeq", cseq);
        }
        resp.set_header("Expires", msg.get_header("Expires").unwrap_or("3600"));
        resp.set_header("Content-Length", "0");

        let expires_sec = msg
            .get_header("Expires")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(3600);

        if expires_sec > 0 {
            let device = GbDevice {
                id: device_id.clone(),
                contact_addr: source,
                expires_at_ms: now_ms.saturating_add(expires_sec * 1000),
                last_keepalive_ms: now_ms,
            };
            self.devices.insert(device_id.clone(), device);
            outputs.push(Gb28181CoreOutput::Event(Gb28181Event::DeviceRegistered {
                device_id,
                contact_addr: source,
            }));
        } else {
            self.devices.remove(&device_id);
            outputs.push(Gb28181CoreOutput::Event(Gb28181Event::DeviceOffline {
                device_id,
            }));
        }

        outputs.push(Gb28181CoreOutput::SendSip(SipSendAction {
            destination: source,
            message: resp,
        }));
    }

    fn handle_message(
        &mut self,
        source: SocketAddr,
        msg: &SipMessage,
        now_ms: u64,
        outputs: &mut Vec<Gb28181CoreOutput>,
    ) {
        let from = msg.get_header("From").unwrap_or("");
        let device_id = extract_sip_username(from).unwrap_or_else(|| "unknown".to_string());

        // Standard Keepalive response
        let mut resp = SipMessage {
            start_line: StartLine::Response {
                version: "SIP/2.0".to_string(),
                status: 200,
                reason: "OK".to_string(),
            },
            headers: Vec::new(),
            body: Vec::new(),
        };

        if let Some(via) = msg.get_header("Via") {
            resp.set_header("Via", via);
        }
        if let Some(from_hdr) = msg.get_header("From") {
            resp.set_header("From", from_hdr);
        }
        if let Some(to_hdr) = msg.get_header("To") {
            resp.set_header("To", to_hdr);
        }
        if let Some(call_id) = msg.get_header("Call-ID") {
            resp.set_header("Call-ID", call_id);
        }
        if let Some(cseq) = msg.get_header("CSeq") {
            resp.set_header("CSeq", cseq);
        }
        resp.set_header("Content-Length", "0");

        if let Some(device) = self.devices.get_mut(&device_id) {
            device.last_keepalive_ms = now_ms;
            outputs.push(Gb28181CoreOutput::Event(Gb28181Event::DeviceKeepalive {
                device_id,
            }));
        }

        outputs.push(Gb28181CoreOutput::SendSip(SipSendAction {
            destination: source,
            message: resp,
        }));
    }

    fn handle_bye(
        &mut self,
        source: SocketAddr,
        msg: &SipMessage,
        outputs: &mut Vec<Gb28181CoreOutput>,
    ) {
        let call_id = msg.get_header("Call-ID").unwrap_or("");
        if let Some((session_id, session)) =
            self.sessions.iter_mut().find(|(_, s)| s.call_id == call_id)
        {
            session.state = DialogState::Terminated;
            outputs.push(Gb28181CoreOutput::Event(Gb28181Event::InviteClosed {
                session_key: session.session_key.clone(),
            }));
            let session_id_clone = session_id.clone();
            self.sessions.remove(&session_id_clone);
        }

        // Return 200 OK
        let mut resp = SipMessage {
            start_line: StartLine::Response {
                version: "SIP/2.0".to_string(),
                status: 200,
                reason: "OK".to_string(),
            },
            headers: Vec::new(),
            body: Vec::new(),
        };
        if let Some(via) = msg.get_header("Via") {
            resp.set_header("Via", via);
        }
        if let Some(from) = msg.get_header("From") {
            resp.set_header("From", from);
        }
        if let Some(to) = msg.get_header("To") {
            resp.set_header("To", to);
        }
        if let Some(call_id_hdr) = msg.get_header("Call-ID") {
            resp.set_header("Call-ID", call_id_hdr);
        }
        if let Some(cseq) = msg.get_header("CSeq") {
            resp.set_header("CSeq", cseq);
        }
        resp.set_header("Content-Length", "0");

        outputs.push(Gb28181CoreOutput::SendSip(SipSendAction {
            destination: source,
            message: resp,
        }));
    }

    fn handle_response(
        &mut self,
        _source: SocketAddr,
        status: u16,
        _reason: &str,
        msg: &SipMessage,
        outputs: &mut Vec<Gb28181CoreOutput>,
    ) {
        let call_id = msg.get_header("Call-ID").unwrap_or("");
        if let Some((_, session)) = self.sessions.iter_mut().find(|(_, s)| s.call_id == call_id) {
            if status == 200 {
                session.state = DialogState::Confirmed;
                outputs.push(Gb28181CoreOutput::Event(Gb28181Event::InviteSuccess {
                    session_key: session.session_key.clone(),
                    ssrc: session.ssrc,
                }));
            }
        }
    }

    fn process_command(
        &mut self,
        cmd: Gb28181Command,
        now_ms: u64,
        outputs: &mut Vec<Gb28181CoreOutput>,
    ) {
        match cmd {
            Gb28181Command::RegisterChallenge {
                device_id,
                destination,
            } => {
                // Generate a 401 response challenge
                let nonce = format!("{:08x}", self.next_call_id_seq.wrapping_mul(0xdead));
                self.next_call_id_seq = self.next_call_id_seq.wrapping_add(1);
                let challenge = SipMessage {
                    start_line: StartLine::Response {
                        version: "SIP/2.0".to_string(),
                        status: 401,
                        reason: "Unauthorized".to_string(),
                    },
                    headers: vec![
                        (
                            "WWW-Authenticate".to_string(),
                            format!("Digest realm=\"cheetah\", nonce=\"{nonce}\", algorithm=MD5"),
                        ),
                        ("Call-ID".to_string(), format!("chal-{device_id}")),
                        ("CSeq".to_string(), "1 REGISTER".to_string()),
                        ("Content-Length".to_string(), "0".to_string()),
                    ],
                    body: Vec::new(),
                };
                outputs.push(Gb28181CoreOutput::SendSip(SipSendAction {
                    destination,
                    message: challenge,
                }));
            }
            Gb28181Command::StartInvite(spec) => {
                let call_id = format!("call-{}", self.next_call_id_seq);
                self.next_call_id_seq += 1;

                let sdp = crate::sdp::GbSdp::to_string(
                    &call_id,
                    &spec.local_ip,
                    spec.local_port,
                    spec.ssrc,
                    spec.is_video,
                    "recvonly",
                );

                let invite = SipMessage {
                    start_line: StartLine::Request {
                        method: "INVITE".to_string(),
                        uri: format!("sip:{}@{}", spec.app_name, spec.destination),
                        version: "SIP/2.0".to_string(),
                    },
                    headers: vec![
                        (
                            "Via".to_string(),
                            format!(
                                "SIP/2.0/UDP {}:{};branch=z9hG4bK{}",
                                spec.local_ip, spec.local_port, spec.ssrc
                            ),
                        ),
                        (
                            "From".to_string(),
                            format!("<sip:{}@{}>", spec.app_name, spec.local_ip),
                        ),
                        (
                            "To".to_string(),
                            format!("<sip:{}@{}>", spec.app_name, spec.local_ip),
                        ),
                        ("Call-ID".to_string(), call_id.clone()),
                        ("CSeq".to_string(), "20 INVITE".to_string()),
                        ("Max-Forwards".to_string(), "70".to_string()),
                        ("User-Agent".to_string(), "Cheetah/0.1".to_string()),
                        (
                            "Contact".to_string(),
                            format!(
                                "<sip:{}@{}:{}>",
                                spec.app_name, spec.local_ip, spec.local_port
                            ),
                        ),
                        ("Content-Type".to_string(), "application/sdp".to_string()),
                        ("Content-Length".to_string(), sdp.len().to_string()),
                    ],
                    body: sdp.into_bytes(),
                };

                let session = GbInviteSession {
                    session_key: spec.session_key.clone(),
                    ssrc: spec.ssrc,
                    destination: spec.destination,
                    call_id: call_id.clone(),
                    state: DialogState::Trying,
                    local_ip: spec.local_ip.clone(),
                    local_port: spec.local_port,
                    app_name: spec.app_name.clone(),
                    _last_activity_ms: now_ms,
                };

                self.sessions.insert(call_id.clone(), session);

                outputs.push(Gb28181CoreOutput::SendSip(SipSendAction {
                    destination: spec.destination,
                    message: invite,
                }));
            }
            Gb28181Command::StopInvite(session_id) => {
                if let Some(session) = self.sessions.remove(&session_id) {
                    let bye = SipMessage {
                        start_line: StartLine::Request {
                            method: "BYE".to_string(),
                            uri: format!("sip:{}@{}", session.app_name, session.destination),
                            version: "SIP/2.0".to_string(),
                        },
                        headers: vec![
                            (
                                "Via".to_string(),
                                format!(
                                    "SIP/2.0/UDP {}:{};branch=z9hG4bK{}-bye",
                                    session.local_ip, session.local_port, session.ssrc
                                ),
                            ),
                            (
                                "From".to_string(),
                                format!(
                                    "<sip:{}@{}>;tag=server1",
                                    session.app_name, session.local_ip
                                ),
                            ),
                            (
                                "To".to_string(),
                                format!("<sip:{}@{}>", session.app_name, session.destination),
                            ),
                            ("Call-ID".to_string(), session.call_id.clone()),
                            ("CSeq".to_string(), "21 BYE".to_string()),
                            ("Max-Forwards".to_string(), "70".to_string()),
                            ("Content-Length".to_string(), "0".to_string()),
                        ],
                        body: Vec::new(),
                    };
                    outputs.push(Gb28181CoreOutput::SendSip(SipSendAction {
                        destination: session.destination,
                        message: bye,
                    }));
                    outputs.push(Gb28181CoreOutput::Event(Gb28181Event::InviteClosed {
                        session_key: session.session_key,
                    }));
                }
            }
            Gb28181Command::StartTalk(spec) => {
                let call_id = format!("talk-{}", self.next_call_id_seq);
                self.next_call_id_seq += 1;

                let sdp = crate::sdp::GbSdp::to_string(
                    &call_id,
                    &spec.local_ip,
                    spec.local_port,
                    spec.ssrc,
                    false, // is_video = false
                    "sendrecv",
                );

                let invite = SipMessage {
                    start_line: StartLine::Request {
                        method: "INVITE".to_string(),
                        uri: format!("sip:{}@{}", spec.app_name, spec.destination),
                        version: "SIP/2.0".to_string(),
                    },
                    headers: vec![
                        (
                            "Via".to_string(),
                            format!(
                                "SIP/2.0/UDP {}:{};branch=z9hG4bK{}",
                                spec.local_ip, spec.local_port, spec.ssrc
                            ),
                        ),
                        (
                            "From".to_string(),
                            format!("<sip:{}@{}>", spec.app_name, spec.local_ip),
                        ),
                        (
                            "To".to_string(),
                            format!("<sip:{}@{}>", spec.app_name, spec.local_ip),
                        ),
                        ("Call-ID".to_string(), call_id.clone()),
                        ("CSeq".to_string(), "20 INVITE".to_string()),
                        ("Max-Forwards".to_string(), "70".to_string()),
                        ("User-Agent".to_string(), "Cheetah/0.1".to_string()),
                        (
                            "Contact".to_string(),
                            format!(
                                "<sip:{}@{}:{}>",
                                spec.app_name, spec.local_ip, spec.local_port
                            ),
                        ),
                        ("Content-Type".to_string(), "application/sdp".to_string()),
                        ("Content-Length".to_string(), sdp.len().to_string()),
                    ],
                    body: sdp.into_bytes(),
                };

                let session = GbInviteSession {
                    session_key: spec.session_key.clone(),
                    ssrc: spec.ssrc,
                    destination: spec.destination,
                    call_id: call_id.clone(),
                    state: DialogState::Trying,
                    local_ip: spec.local_ip.clone(),
                    local_port: spec.local_port,
                    app_name: spec.app_name.clone(),
                    _last_activity_ms: now_ms,
                };

                self.sessions.insert(call_id.clone(), session);

                outputs.push(Gb28181CoreOutput::SendSip(SipSendAction {
                    destination: spec.destination,
                    message: invite,
                }));
            }
            Gb28181Command::StopTalk(session_id) => {
                if let Some(session) = self.sessions.remove(&session_id) {
                    let bye = SipMessage {
                        start_line: StartLine::Request {
                            method: "BYE".to_string(),
                            uri: format!("sip:{}@{}", session.app_name, session.destination),
                            version: "SIP/2.0".to_string(),
                        },
                        headers: vec![
                            (
                                "Via".to_string(),
                                format!(
                                    "SIP/2.0/UDP {}:{};branch=z9hG4bK{}-bye",
                                    session.local_ip, session.local_port, session.ssrc
                                ),
                            ),
                            (
                                "From".to_string(),
                                format!(
                                    "<sip:{}@{}>;tag=server1",
                                    session.app_name, session.local_ip
                                ),
                            ),
                            (
                                "To".to_string(),
                                format!("<sip:{}@{}>", session.app_name, session.destination),
                            ),
                            ("Call-ID".to_string(), session.call_id.clone()),
                            ("CSeq".to_string(), "21 BYE".to_string()),
                            ("Max-Forwards".to_string(), "70".to_string()),
                            ("Content-Length".to_string(), "0".to_string()),
                        ],
                        body: Vec::new(),
                    };
                    outputs.push(Gb28181CoreOutput::SendSip(SipSendAction {
                        destination: session.destination,
                        message: bye,
                    }));
                    outputs.push(Gb28181CoreOutput::Event(Gb28181Event::InviteClosed {
                        session_key: session.session_key,
                    }));
                }
            }
        }
    }

    fn check_timeouts(&mut self, now_ms: u64, outputs: &mut Vec<Gb28181CoreOutput>) {
        let mut offline_devices = Vec::with_capacity(self.devices.len());
        for (id, dev) in &self.devices {
            if now_ms > dev.expires_at_ms {
                offline_devices.push(id.clone());
            }
        }

        for id in offline_devices {
            self.devices.remove(&id);
            outputs.push(Gb28181CoreOutput::Event(Gb28181Event::DeviceOffline {
                device_id: id.clone(),
            }));
            outputs.push(Gb28181CoreOutput::Diagnostic(
                Gb28181Diagnostic::KeepaliveTimeout { device_id: id },
            ));
        }
    }

    pub fn list_devices(&self) -> Vec<GbDevice> {
        self.devices.values().cloned().collect()
    }
}

fn extract_sip_username(from_hdr: &str) -> Option<String> {
    // Extract `34020000001320000001` from `<sip:34020000001320000001@3402000000>;tag=...`
    let start = from_hdr.find("sip:")?;
    let content = &from_hdr[start + 4..];
    let end = content.find('@')?;
    Some(content[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::SipMessage;

    #[test]
    fn test_core_device_registration_and_keepalive() {
        let mut core = Gb28181Core::new();
        let mut outputs = Vec::new();

        let req_str = "REGISTER sip:34020000002000000001@3402000000 SIP/2.0\r\n\
                       Via: SIP/2.0/UDP 192.168.1.100:5060;branch=z9hG4bK123\r\n\
                       From: <sip:34020000001320000001@3402000000>;tag=abc\r\n\
                       To: <sip:34020000002000000001@3402000000>\r\n\
                       Call-ID: call-9999\r\n\
                       CSeq: 1 REGISTER\r\n\
                       Expires: 3600\r\n\
                       Content-Length: 0\r\n\
                       \r\n";

        let msg = SipMessage::parse(req_str).unwrap();
        let source = "192.168.1.100:5060".parse::<SocketAddr>().unwrap();

        core.handle_input(
            Gb28181CoreInput::SipMessage {
                source,
                message: msg,
            },
            &mut outputs,
        );

        assert_eq!(outputs.len(), 2);
        if let Gb28181CoreOutput::Event(Gb28181Event::DeviceRegistered {
            device_id,
            contact_addr,
        }) = &outputs[0]
        {
            assert_eq!(device_id, "34020000001320000001");
            assert_eq!(*contact_addr, source);
        } else {
            panic!("Expected registration event");
        }

        assert_eq!(core.list_devices().len(), 1);
    }

    #[test]
    fn test_core_voice_talk() {
        let mut core = Gb28181Core::new();
        let mut outputs = Vec::new();

        let spec = GbTalkSpec {
            session_key: "talk_test".to_string(),
            ssrc: 987654321,
            destination: "127.0.0.1:5060".parse().unwrap(),
            app_name: "live".to_string(),
            stream_name: "talk1".to_string(),
            local_ip: "192.168.1.10".to_string(),
            local_port: 30000,
        };

        core.handle_input(
            Gb28181CoreInput::Command(Gb28181Command::StartTalk(spec)),
            &mut outputs,
        );

        assert_eq!(outputs.len(), 1);
        if let Gb28181CoreOutput::SendSip(action) = &outputs[0] {
            assert_eq!(
                action.destination,
                "127.0.0.1:5060".parse::<SocketAddr>().unwrap()
            );
            assert_eq!(
                action.message.start_line,
                StartLine::Request {
                    method: "INVITE".to_string(),
                    uri: "sip:live@127.0.0.1:5060".to_string(),
                    version: "SIP/2.0".to_string(),
                }
            );
            let body_str = std::str::from_utf8(&action.message.body).unwrap();
            assert!(body_str.contains("m=audio 30000 RTP/AVP 8"));
            assert!(body_str.contains("a=rtpmap:8 PCMA/8000"));
            assert!(body_str.contains("a=sendrecv"));
            assert!(body_str.contains("a=y:0987654321"));
        } else {
            panic!("Expected SendSip");
        }

        // Test StopTalk
        let call_id = core.sessions.keys().next().unwrap().clone();
        outputs.clear();
        core.handle_input(
            Gb28181CoreInput::Command(Gb28181Command::StopTalk(call_id)),
            &mut outputs,
        );

        assert_eq!(outputs.len(), 2);
        if let Gb28181CoreOutput::SendSip(action) = &outputs[0] {
            assert_eq!(
                action.message.start_line,
                StartLine::Request {
                    method: "BYE".to_string(),
                    uri: "sip:live@127.0.0.1:5060".to_string(),
                    version: "SIP/2.0".to_string(),
                }
            );
        } else {
            panic!("Expected SendSip BYE");
        }
        if let Gb28181CoreOutput::Event(Gb28181Event::InviteClosed { session_key }) = &outputs[1] {
            assert_eq!(session_key, "talk_test");
        } else {
            panic!("Expected InviteClosed event");
        }
    }
}
