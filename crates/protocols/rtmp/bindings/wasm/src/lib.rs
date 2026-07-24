#![expect(clippy::missing_safety_doc)]

use std::alloc::Layout;

use base64::Engine;
use cheetah_rtmp_core::{CoreInput, RtmpCore, RtmpCoreCommand, RtmpEvent, RtmpMediaType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
/// JSON commands sent from the host into the WASM RTMP core.
///
/// 从宿主发送到 WASM RTMP core 的 JSON 命令。
enum JsonCommand {
    Timeout {
        timer_id: u64,
    },
    AcceptPublish {
        stream_id: u32,
    },
    RejectPublish {
        stream_id: u32,
        description: String,
    },
    AcceptPlay {
        stream_id: u32,
    },
    AcceptPlayConfigured {
        stream_id: u32,
        emit_play_status: bool,
        emit_sample_access: bool,
    },
    RejectPlay {
        stream_id: u32,
        description: String,
    },
    SendMetadata {
        stream_id: u32,
        timestamp_ms: u32,
        payload_base64: String,
    },
    SendAudio {
        stream_id: u32,
        timestamp_ms: u32,
        payload_base64: String,
    },
    SendVideo {
        stream_id: u32,
        timestamp_ms: u32,
        payload_base64: String,
    },
    SendNotify {
        stream_id: u32,
        timestamp_ms: u32,
        payload_base64: String,
    },
    CloseStream {
        stream_id: u32,
    },
    CloseConnection,
}

#[derive(Debug, Serialize)]
/// JSON representation of a core output, returned to the host.
///
/// 返回给宿主的 core 输出 JSON 表示。
struct JsonOutput {
    kind: &'static str,
    timer_id: u64,
    at_micros: u64,
    stream_id: u32,
    timestamp_ms: u32,
    media_type: &'static str,
    primary_base64: Option<String>,
    secondary_base64: Option<String>,
    primary_text: Option<String>,
    secondary_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Internal classification of output kinds for JSON serialization.
///
/// 用于 JSON 序列化的输出类型内部分类。
enum OutputKind {
    Write,
    EventConnected,
    EventPublishRequested,
    EventPlayRequested,
    EventMetadata,
    EventNotify,
    EventMediaData,
    EventStreamClosed,
    EventPeerClosed,
    EventStreamCreated,
    EventCommandIgnored,
    EventMessageIgnored,
    EventUserControlIgnored,
    EventAckReceived,
    EventLocalAckWindowUpdated,
    EventPeerAckWindowUpdated,
    EventClientStateChanged,
    EventClientDisconnectRequested,
    SetTimer,
    CancelTimer,
}

/// Map an `OutputKind` to its snake_case JSON string.
///
/// 将 `OutputKind` 映射为 snake_case JSON 字符串。
fn output_kind_name(kind: OutputKind) -> &'static str {
    match kind {
        OutputKind::Write => "write",
        OutputKind::EventConnected => "event_connected",
        OutputKind::EventPublishRequested => "event_publish_requested",
        OutputKind::EventPlayRequested => "event_play_requested",
        OutputKind::EventMetadata => "event_metadata",
        OutputKind::EventNotify => "event_notify",
        OutputKind::EventMediaData => "event_media_data",
        OutputKind::EventStreamClosed => "event_stream_closed",
        OutputKind::EventPeerClosed => "event_peer_closed",
        OutputKind::EventStreamCreated => "event_stream_created",
        OutputKind::EventCommandIgnored => "event_command_ignored",
        OutputKind::EventMessageIgnored => "event_message_ignored",
        OutputKind::EventUserControlIgnored => "event_user_control_ignored",
        OutputKind::EventAckReceived => "event_ack_received",
        OutputKind::EventLocalAckWindowUpdated => "event_local_ack_window_updated",
        OutputKind::EventPeerAckWindowUpdated => "event_peer_ack_window_updated",
        OutputKind::EventClientStateChanged => "event_client_state_changed",
        OutputKind::EventClientDisconnectRequested => "event_client_disconnect_requested",
        OutputKind::SetTimer => "set_timer",
        OutputKind::CancelTimer => "cancel_timer",
    }
}

/// Map an internal `RtmpMediaType` to a JSON string.
///
/// 将内部 `RtmpMediaType` 映射为 JSON 字符串。
fn media_type_name(media_type: RtmpMediaType) -> &'static str {
    match media_type {
        RtmpMediaType::Audio => "audio",
        RtmpMediaType::Video => "video",
        RtmpMediaType::Data => "data",
    }
}

#[unsafe(no_mangle)]
/// Allocate a raw memory block of the given size for the host.
///
/// 为宿主分配指定大小的原始内存块。
pub extern "C" fn rtmp_alloc(size: u32) -> *mut u8 {
    if size == 0 {
        return std::ptr::null_mut();
    }
    let Ok(layout) = Layout::from_size_align(size as usize, 1) else {
        return std::ptr::null_mut();
    };
    unsafe { std::alloc::alloc(layout) }
}

#[unsafe(no_mangle)]
/// Free a raw memory block previously allocated by `rtmp_alloc`.
///
/// 释放之前由 `rtmp_alloc` 分配的原始内存块。
pub unsafe extern "C" fn rtmp_free(ptr: *mut u8, size: u32) {
    if ptr.is_null() || size == 0 {
        return;
    }
    let Ok(layout) = Layout::from_size_align(size as usize, 1) else {
        return;
    };
    unsafe { std::alloc::dealloc(ptr, layout) };
}

#[unsafe(no_mangle)]
/// Return a pointer to the data of a `Vec<u8>` owned by Rust.
///
/// 返回 Rust 拥有的 `Vec<u8>` 数据指针。
pub unsafe extern "C" fn rtmp_vec_ptr(v: *const Vec<u8>) -> *const u8 {
    if v.is_null() {
        return std::ptr::null();
    }
    unsafe { (*v).as_ptr() }
}

#[unsafe(no_mangle)]
/// Return the length of a `Vec<u8>` owned by Rust.
///
/// 返回 Rust 拥有的 `Vec<u8>` 长度。
pub unsafe extern "C" fn rtmp_vec_len(v: *const Vec<u8>) -> u32 {
    if v.is_null() {
        return 0;
    }
    unsafe { u32::try_from((*v).len()).unwrap_or(u32::MAX) }
}

#[unsafe(no_mangle)]
/// Free a `Vec<u8>` previously returned by the WASM binding.
///
/// 释放之前由 WASM 绑定返回的 `Vec<u8>`。
pub unsafe extern "C" fn rtmp_vec_free(v: *mut Vec<u8>) {
    if v.is_null() {
        return;
    }
    let _ = unsafe { Box::from_raw(v) };
}

/// Read a raw pointer/length pair as a byte slice, if non-null.
///
/// 若非空，则将原始指针/长度对读取为字节切片。
unsafe fn read_bytes<'a>(json_bytes: *const u8, json_bytes_len: u32) -> Option<&'a [u8]> {
    if json_bytes.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(json_bytes, json_bytes_len as usize) })
}

/// Decode a base64 payload into bytes for core media commands.
///
/// 将 base64 负载解码为字节，用于 core 媒体命令。
fn decode_payload_base64(payload_base64: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(payload_base64)
        .ok()
}

/// Opaque handle that owns an `RtmpCore` and a queue of JSON outputs.
///
/// 拥有 `RtmpCore` 与 JSON 输出队列的不透明句柄。
pub struct WasmHandle {
    core: RtmpCore,
    output_queue: std::collections::VecDeque<JsonOutput>,
}

impl WasmHandle {
    /// Create a new handle with an empty core and output queue.
    ///
    /// 用空的 core 与输出队列创建新句柄。
    fn new() -> Self {
        Self {
            core: RtmpCore::new(),
            output_queue: std::collections::VecDeque::new(),
        }
    }

    /// Push an input into the core and queue the converted JSON outputs.
    ///
    /// 将输入推入 core 并将转换后的 JSON 输出入队。
    fn apply_input(&mut self, input: CoreInput) {
        if let Ok(outputs) = self.core.handle_input(input) {
            for output in outputs {
                self.output_queue.push_back(convert_output(output));
            }
        }
    }
}

/// Convert a `CoreOutput` into a `JsonOutput` for the host.
///
/// 将 `CoreOutput` 转换为给宿主的 `JsonOutput`。
fn convert_output(output: cheetah_rtmp_core::CoreOutput) -> JsonOutput {
    use cheetah_rtmp_core::CoreOutput;

    match output {
        CoreOutput::Write(payload) => JsonOutput {
            kind: output_kind_name(OutputKind::Write),
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: Some(base64::engine::general_purpose::STANDARD.encode(&payload)),
            secondary_base64: None,
            primary_text: std::str::from_utf8(&payload).ok().map(ToOwned::to_owned),
            secondary_text: None,
        },
        CoreOutput::SetTimer { id, at_micros } => JsonOutput {
            kind: output_kind_name(OutputKind::SetTimer),
            timer_id: id,
            at_micros,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: None,
            secondary_text: None,
        },
        CoreOutput::CancelTimer { id } => JsonOutput {
            kind: output_kind_name(OutputKind::CancelTimer),
            timer_id: id,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: None,
            secondary_text: None,
        },
        CoreOutput::Event(event) => convert_event(event),
    }
}

/// Convert an `RtmpEvent` into a `JsonOutput` for the host.
///
/// 将 `RtmpEvent` 转换为给宿主的 `JsonOutput`。
fn convert_event(event: RtmpEvent) -> JsonOutput {
    match event {
        RtmpEvent::Connected { app, .. } => JsonOutput {
            kind: output_kind_name(OutputKind::EventConnected),
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: Some(app),
            secondary_text: None,
        },
        RtmpEvent::PublishRequested {
            stream_id,
            app,
            stream_name,
            ..
        } => JsonOutput {
            kind: output_kind_name(OutputKind::EventPublishRequested),
            timer_id: 0,
            at_micros: 0,
            stream_id,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: Some(app),
            secondary_text: Some(stream_name),
        },
        RtmpEvent::PlayRequested {
            stream_id,
            app,
            stream_name,
            ..
        } => JsonOutput {
            kind: output_kind_name(OutputKind::EventPlayRequested),
            timer_id: 0,
            at_micros: 0,
            stream_id,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: Some(app),
            secondary_text: Some(stream_name),
        },
        RtmpEvent::StreamCreated { stream_id } => JsonOutput {
            kind: output_kind_name(OutputKind::EventStreamCreated),
            timer_id: 0,
            at_micros: 0,
            stream_id,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: None,
            secondary_text: None,
        },
        RtmpEvent::CommandIgnored { name, detail } => JsonOutput {
            kind: output_kind_name(OutputKind::EventCommandIgnored),
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: Some(name),
            secondary_text: Some(detail),
        },
        RtmpEvent::MessageIgnored { name, detail } => JsonOutput {
            kind: output_kind_name(OutputKind::EventMessageIgnored),
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: Some(name),
            secondary_text: Some(detail),
        },
        RtmpEvent::UserControlIgnored { name, detail } => JsonOutput {
            kind: output_kind_name(OutputKind::EventUserControlIgnored),
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: Some(name),
            secondary_text: Some(detail),
        },
        RtmpEvent::AckReceived { sequence_number } => JsonOutput {
            kind: output_kind_name(OutputKind::EventAckReceived),
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: Some(format!("{sequence_number}")),
            secondary_text: None,
        },
        RtmpEvent::LocalAckWindowUpdated { size } => JsonOutput {
            kind: output_kind_name(OutputKind::EventLocalAckWindowUpdated),
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: Some(format!("{size}")),
            secondary_text: None,
        },
        RtmpEvent::PeerAckWindowUpdated { size } => JsonOutput {
            kind: output_kind_name(OutputKind::EventPeerAckWindowUpdated),
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: Some(format!("{size}")),
            secondary_text: None,
        },
        RtmpEvent::ClientStateChanged { .. } => JsonOutput {
            kind: output_kind_name(OutputKind::EventClientStateChanged),
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: None,
            secondary_text: None,
        },
        RtmpEvent::ClientDisconnectRequested { reason } => JsonOutput {
            kind: output_kind_name(OutputKind::EventClientDisconnectRequested),
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: Some(reason),
            secondary_text: None,
        },
        RtmpEvent::Metadata { stream_id, values } => {
            let mut payload = Vec::new();
            for value in &values {
                cheetah_rtmp_core::AmfValue::encode(value, &mut payload);
            }
            JsonOutput {
                kind: output_kind_name(OutputKind::EventMetadata),
                timer_id: 0,
                at_micros: 0,
                stream_id,
                timestamp_ms: 0,
                media_type: "none",
                primary_base64: Some(base64::engine::general_purpose::STANDARD.encode(&payload)),
                secondary_base64: None,
                primary_text: None,
                secondary_text: None,
            }
        }
        RtmpEvent::Notify {
            stream_id,
            name,
            values,
        } => {
            let mut payload = Vec::new();
            for value in &values {
                cheetah_rtmp_core::AmfValue::encode(value, &mut payload);
            }
            JsonOutput {
                kind: output_kind_name(OutputKind::EventNotify),
                timer_id: 0,
                at_micros: 0,
                stream_id,
                timestamp_ms: 0,
                media_type: "none",
                primary_base64: None,
                secondary_base64: Some(base64::engine::general_purpose::STANDARD.encode(&payload)),
                primary_text: Some(name),
                secondary_text: None,
            }
        }
        RtmpEvent::MediaData {
            stream_id,
            timestamp_ms,
            media_type,
            payload,
        } => JsonOutput {
            kind: output_kind_name(OutputKind::EventMediaData),
            timer_id: 0,
            at_micros: 0,
            stream_id,
            timestamp_ms,
            media_type: media_type_name(media_type),
            primary_base64: Some(base64::engine::general_purpose::STANDARD.encode(&payload)),
            secondary_base64: None,
            primary_text: std::str::from_utf8(&payload).ok().map(ToOwned::to_owned),
            secondary_text: None,
        },
        RtmpEvent::StreamClosed { stream_id } => JsonOutput {
            kind: output_kind_name(OutputKind::EventStreamClosed),
            timer_id: 0,
            at_micros: 0,
            stream_id,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: None,
            secondary_text: None,
        },
        RtmpEvent::PeerClosed => JsonOutput {
            kind: output_kind_name(OutputKind::EventPeerClosed),
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: None,
            secondary_text: None,
        },
        // Player-control events introduced after the WASM JSON contract froze. Surfacing them
        // as `kind: "none"` keeps the wire format stable; promote each to a dedicated kind in
        // the next breaking JSON revision when the JS bindings are ready to consume them.
        RtmpEvent::SeekRequested { .. }
        | RtmpEvent::PauseRequested { .. }
        | RtmpEvent::ReceiveVideo { .. }
        | RtmpEvent::ReceiveAudio { .. } => JsonOutput {
            kind: "none",
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: "none",
            primary_base64: None,
            secondary_base64: None,
            primary_text: None,
            secondary_text: None,
        },
    }
}

#[unsafe(no_mangle)]
/// Allocate a new WASM RTMP handle.
///
/// 分配新的 WASM RTMP 句柄。
pub unsafe extern "C" fn rtmp_wasm_new() -> *mut WasmHandle {
    Box::into_raw(Box::new(WasmHandle::new()))
}

#[unsafe(no_mangle)]
/// Free a WASM RTMP handle previously created by `rtmp_wasm_new`.
///
/// 释放之前由 `rtmp_wasm_new` 创建的 WASM RTMP 句柄。
pub unsafe extern "C" fn rtmp_wasm_free(handle: *mut WasmHandle) {
    if handle.is_null() {
        return;
    }
    let _ = unsafe { Box::from_raw(handle) };
}

#[unsafe(no_mangle)]
/// Feed raw bytes into the core.
///
/// 将原始字节喂入 core。
pub unsafe extern "C" fn rtmp_wasm_handle_bytes(
    handle: *mut WasmHandle,
    data: *const u8,
    len: u32,
) -> u32 {
    let Some(handle) = (unsafe { handle_mut(handle) }) else {
        return 1;
    };
    let Some(bytes) = (unsafe { read_bytes(data, len) }) else {
        return 1;
    };
    handle.apply_input(CoreInput::Bytes(bytes.to_vec().into()));
    0
}

#[unsafe(no_mangle)]
/// Notify the core that a timer has expired.
///
/// 通知 core 定时器已到期。
pub unsafe extern "C" fn rtmp_wasm_handle_timeout(handle: *mut WasmHandle, timer_id: u64) -> u32 {
    let Some(handle) = (unsafe { handle_mut(handle) }) else {
        return 1;
    };
    handle.apply_input(CoreInput::Timeout { id: timer_id });
    0
}

#[unsafe(no_mangle)]
/// Parse a JSON command and feed the resulting core input.
///
/// 解析 JSON 命令并喂入对应的 core 输入。
pub unsafe extern "C" fn rtmp_wasm_handle_command_json(
    handle: *mut WasmHandle,
    json_bytes: *const u8,
    json_bytes_len: u32,
) -> u32 {
    let Some(handle) = (unsafe { handle_mut(handle) }) else {
        return 1;
    };
    let Some(bytes) = (unsafe { read_bytes(json_bytes, json_bytes_len) }) else {
        return 1;
    };
    let Ok(command) = serde_json::from_slice::<JsonCommand>(bytes) else {
        return 2;
    };

    let input = match command {
        JsonCommand::Timeout { timer_id } => CoreInput::Timeout { id: timer_id },
        JsonCommand::AcceptPublish { stream_id } => {
            CoreInput::Command(RtmpCoreCommand::AcceptPublish { stream_id })
        }
        JsonCommand::RejectPublish {
            stream_id,
            description,
        } => CoreInput::Command(RtmpCoreCommand::RejectPublish {
            stream_id,
            description,
        }),
        JsonCommand::AcceptPlay { stream_id } => {
            CoreInput::Command(RtmpCoreCommand::AcceptPlay { stream_id })
        }
        JsonCommand::AcceptPlayConfigured {
            stream_id,
            emit_play_status,
            emit_sample_access,
        } => CoreInput::Command(RtmpCoreCommand::AcceptPlayConfigured {
            stream_id,
            emit_play_status,
            emit_sample_access,
        }),
        JsonCommand::RejectPlay {
            stream_id,
            description,
        } => CoreInput::Command(RtmpCoreCommand::RejectPlay {
            stream_id,
            description,
        }),
        JsonCommand::SendMetadata {
            stream_id,
            timestamp_ms,
            payload_base64,
        } => {
            let Some(payload) = decode_payload_base64(&payload_base64) else {
                return 2;
            };
            CoreInput::Command(RtmpCoreCommand::SendMetadata {
                stream_id,
                timestamp_ms,
                payload: payload.into(),
            })
        }
        JsonCommand::SendAudio {
            stream_id,
            timestamp_ms,
            payload_base64,
        } => {
            let Some(payload) = decode_payload_base64(&payload_base64) else {
                return 2;
            };
            CoreInput::Command(RtmpCoreCommand::SendAudio {
                stream_id,
                timestamp_ms,
                payload: payload.into(),
            })
        }
        JsonCommand::SendVideo {
            stream_id,
            timestamp_ms,
            payload_base64,
        } => {
            let Some(payload) = decode_payload_base64(&payload_base64) else {
                return 2;
            };
            CoreInput::Command(RtmpCoreCommand::SendVideo {
                stream_id,
                timestamp_ms,
                payload: payload.into(),
            })
        }
        JsonCommand::SendNotify {
            stream_id,
            timestamp_ms,
            payload_base64,
        } => {
            let Some(payload) = decode_payload_base64(&payload_base64) else {
                return 2;
            };
            CoreInput::Command(RtmpCoreCommand::SendNotify {
                stream_id,
                timestamp_ms,
                payload: payload.into(),
            })
        }
        JsonCommand::CloseStream { stream_id } => {
            CoreInput::Command(RtmpCoreCommand::CloseStream { stream_id })
        }
        JsonCommand::CloseConnection => CoreInput::Command(RtmpCoreCommand::CloseConnection),
    };

    handle.apply_input(input);
    0
}

/// Dereference a handle pointer, returning `None` if null.
///
/// 解引用句柄指针，若为空则返回 `None`。
unsafe fn handle_mut<'a>(handle: *mut WasmHandle) -> Option<&'a mut WasmHandle> {
    if handle.is_null() {
        return None;
    }
    Some(unsafe { &mut *handle })
}

#[unsafe(no_mangle)]
/// Return the number of JSON outputs queued on the handle.
///
/// 返回句柄上已排队的 JSON 输出数量。
pub unsafe extern "C" fn rtmp_wasm_pending_output_count(handle: *const WasmHandle) -> u32 {
    if handle.is_null() {
        return 0;
    }
    let handle_ref = unsafe { &*handle };
    u32::try_from(handle_ref.output_queue.len()).unwrap_or(u32::MAX)
}

#[unsafe(no_mangle)]
/// Pop the next JSON output and serialize it into a `Vec<u8>` owned by Rust.
///
/// 弹出下一个 JSON 输出并序列化为 Rust 拥有的 `Vec<u8>`。
pub unsafe extern "C" fn rtmp_wasm_next_output_json(handle: *mut WasmHandle) -> *mut Vec<u8> {
    let Some(handle) = (unsafe { handle_mut(handle) }) else {
        return std::ptr::null_mut();
    };

    let Some(output) = handle.output_queue.pop_front() else {
        return std::ptr::null_mut();
    };

    match serde_json::to_vec(&output) {
        Ok(bytes) => Box::into_raw(Box::new(bytes)),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_command_bridge_generates_json_output() {
        let handle = unsafe { rtmp_wasm_new() };
        let cmd = br#"{"type":"accept_play","stream_id":1}"#;
        let status =
            unsafe { rtmp_wasm_handle_command_json(handle, cmd.as_ptr(), cmd.len() as u32) };
        assert_eq!(status, 0);

        let count = unsafe { rtmp_wasm_pending_output_count(handle) };
        assert!(count >= 1);

        let output_vec = unsafe { rtmp_wasm_next_output_json(handle) };
        assert!(!output_vec.is_null());
        unsafe { rtmp_vec_free(output_vec) };
        unsafe { rtmp_wasm_free(handle) };
    }
}
