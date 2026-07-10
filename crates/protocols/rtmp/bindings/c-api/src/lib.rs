#![expect(clippy::missing_safety_doc)]

use std::collections::VecDeque;
use std::ffi::{c_char, CString};

use bytes::Bytes;
use cheetah_rtmp_core::{
    amf::AmfValue, CoreInput, CoreOutput, RtmpCore, RtmpCoreCommand, RtmpEvent, RtmpMediaType,
};

const EMPTY_ERROR: &[u8] = b"\0";
const NULL_POINTER_ERROR: &[u8] = b"null pointer\0";

/// Error returned by `RTMP Core API` operations.
/// `RTMP Core API` 操作返回的错误。
#[repr(C)]
#[expect(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpCoreApiError {
    RTMP_CORE_API_ERROR_OK = 0,
    RTMP_CORE_API_ERROR_INVALID_ARGUMENT,
    RTMP_CORE_API_ERROR_NULL_POINTER,
    RTMP_CORE_API_ERROR_CORE,
    RTMP_CORE_API_ERROR_NO_OUTPUT,
    RTMP_CORE_API_ERROR_OVERFLOW,
}

/// Kind of `RTMP Core Output`.
/// `RTMP Core Output` 的种类。
#[repr(C)]
#[expect(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpCoreOutputKind {
    RTMP_CORE_OUTPUT_KIND_NONE = 0,
    RTMP_CORE_OUTPUT_KIND_WRITE,
    RTMP_CORE_OUTPUT_KIND_EVENT_CONNECTED,
    RTMP_CORE_OUTPUT_KIND_EVENT_STREAM_CREATED,
    RTMP_CORE_OUTPUT_KIND_EVENT_COMMAND_IGNORED,
    RTMP_CORE_OUTPUT_KIND_EVENT_MESSAGE_IGNORED,
    RTMP_CORE_OUTPUT_KIND_EVENT_USER_CONTROL_IGNORED,
    RTMP_CORE_OUTPUT_KIND_EVENT_ACK_RECEIVED,
    RTMP_CORE_OUTPUT_KIND_EVENT_LOCAL_ACK_WINDOW_UPDATED,
    RTMP_CORE_OUTPUT_KIND_EVENT_PEER_ACK_WINDOW_UPDATED,
    RTMP_CORE_OUTPUT_KIND_EVENT_CLIENT_STATE_CHANGED,
    RTMP_CORE_OUTPUT_KIND_EVENT_CLIENT_DISCONNECT_REQUESTED,
    RTMP_CORE_OUTPUT_KIND_EVENT_PUBLISH_REQUESTED,
    RTMP_CORE_OUTPUT_KIND_EVENT_PLAY_REQUESTED,
    RTMP_CORE_OUTPUT_KIND_EVENT_METADATA,
    RTMP_CORE_OUTPUT_KIND_EVENT_NOTIFY,
    RTMP_CORE_OUTPUT_KIND_EVENT_MEDIA_DATA,
    RTMP_CORE_OUTPUT_KIND_EVENT_STREAM_CLOSED,
    RTMP_CORE_OUTPUT_KIND_EVENT_PEER_CLOSED,
    RTMP_CORE_OUTPUT_KIND_SET_TIMER,
    RTMP_CORE_OUTPUT_KIND_CANCEL_TIMER,
}

/// Type of `RTMP Core Output Media`.
/// `RTMP Core Output Media` 的类型。
#[repr(C)]
#[expect(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpCoreOutputMediaType {
    RTMP_CORE_OUTPUT_MEDIA_TYPE_NONE = 0,
    RTMP_CORE_OUTPUT_MEDIA_TYPE_AUDIO,
    RTMP_CORE_OUTPUT_MEDIA_TYPE_VIDEO,
    RTMP_CORE_OUTPUT_MEDIA_TYPE_DATA,
}

/// View of `RTMP Core Output`.
/// `RTMP Core Output` 的视图。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RtmpCoreOutputView {
    pub kind: RtmpCoreOutputKind,
    pub timer_id: u64,
    pub at_micros: u64,
    pub stream_id: u32,
    pub timestamp_ms: u32,
    pub media_type: RtmpCoreOutputMediaType,
    pub primary_ptr: *const u8,
    pub primary_len: u32,
    pub secondary_ptr: *const u8,
    pub secondary_len: u32,
}

impl Default for RtmpCoreOutputView {
    fn default() -> Self {
        Self {
            kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_NONE,
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: RtmpCoreOutputMediaType::RTMP_CORE_OUTPUT_MEDIA_TYPE_NONE,
            primary_ptr: std::ptr::null(),
            primary_len: 0,
            secondary_ptr: std::ptr::null(),
            secondary_len: 0,
        }
    }
}

struct OwnedOutput {
    kind: RtmpCoreOutputKind,
    timer_id: u64,
    at_micros: u64,
    stream_id: u32,
    timestamp_ms: u32,
    media_type: RtmpCoreOutputMediaType,
    primary: Bytes,
    secondary: Bytes,
}

impl OwnedOutput {
    fn empty(kind: RtmpCoreOutputKind) -> Self {
        Self {
            kind,
            timer_id: 0,
            at_micros: 0,
            stream_id: 0,
            timestamp_ms: 0,
            media_type: RtmpCoreOutputMediaType::RTMP_CORE_OUTPUT_MEDIA_TYPE_NONE,
            primary: Bytes::new(),
            secondary: Bytes::new(),
        }
    }

    fn from_core(output: CoreOutput) -> Self {
        match output {
            CoreOutput::Write(payload) => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_WRITE,
                timer_id: 0,
                at_micros: 0,
                stream_id: 0,
                timestamp_ms: 0,
                media_type: RtmpCoreOutputMediaType::RTMP_CORE_OUTPUT_MEDIA_TYPE_NONE,
                primary: payload,
                secondary: Bytes::new(),
            },
            CoreOutput::SetTimer { id, at_micros } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_SET_TIMER,
                timer_id: id,
                at_micros,
                stream_id: 0,
                timestamp_ms: 0,
                media_type: RtmpCoreOutputMediaType::RTMP_CORE_OUTPUT_MEDIA_TYPE_NONE,
                primary: Bytes::new(),
                secondary: Bytes::new(),
            },
            CoreOutput::CancelTimer { id } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_CANCEL_TIMER,
                timer_id: id,
                at_micros: 0,
                stream_id: 0,
                timestamp_ms: 0,
                media_type: RtmpCoreOutputMediaType::RTMP_CORE_OUTPUT_MEDIA_TYPE_NONE,
                primary: Bytes::new(),
                secondary: Bytes::new(),
            },
            CoreOutput::Event(event) => Self::from_event(event),
        }
    }

    fn from_event(event: RtmpEvent) -> Self {
        match event {
            RtmpEvent::Connected { app, .. } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_CONNECTED,
                primary: Bytes::from(app.into_bytes()),
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_CONNECTED)
            },
            RtmpEvent::PublishRequested {
                stream_id,
                app,
                stream_name,
                ..
            } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_PUBLISH_REQUESTED,
                stream_id,
                primary: Bytes::from(app.into_bytes()),
                secondary: Bytes::from(stream_name.into_bytes()),
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_PUBLISH_REQUESTED)
            },
            RtmpEvent::PlayRequested {
                stream_id,
                app,
                stream_name,
                ..
            } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_PLAY_REQUESTED,
                stream_id,
                primary: Bytes::from(app.into_bytes()),
                secondary: Bytes::from(stream_name.into_bytes()),
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_PLAY_REQUESTED)
            },
            RtmpEvent::StreamCreated { stream_id } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_STREAM_CREATED,
                stream_id,
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_STREAM_CREATED)
            },
            RtmpEvent::CommandIgnored { name, detail } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_COMMAND_IGNORED,
                primary: Bytes::from(name.into_bytes()),
                secondary: Bytes::from(detail.into_bytes()),
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_COMMAND_IGNORED)
            },
            RtmpEvent::MessageIgnored { name, detail } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_MESSAGE_IGNORED,
                primary: Bytes::from(name.into_bytes()),
                secondary: Bytes::from(detail.into_bytes()),
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_MESSAGE_IGNORED)
            },
            RtmpEvent::UserControlIgnored { name, detail } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_USER_CONTROL_IGNORED,
                primary: Bytes::from(name.into_bytes()),
                secondary: Bytes::from(detail.into_bytes()),
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_USER_CONTROL_IGNORED)
            },
            RtmpEvent::AckReceived { sequence_number } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_ACK_RECEIVED,
                primary: Bytes::from(sequence_number.to_be_bytes().to_vec()),
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_ACK_RECEIVED)
            },
            RtmpEvent::LocalAckWindowUpdated { size } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_LOCAL_ACK_WINDOW_UPDATED,
                primary: Bytes::from(size.to_be_bytes().to_vec()),
                ..Self::empty(
                    RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_LOCAL_ACK_WINDOW_UPDATED,
                )
            },
            RtmpEvent::PeerAckWindowUpdated { size } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_PEER_ACK_WINDOW_UPDATED,
                primary: Bytes::from(size.to_be_bytes().to_vec()),
                ..Self::empty(
                    RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_PEER_ACK_WINDOW_UPDATED,
                )
            },
            RtmpEvent::ClientStateChanged { .. } => {
                Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_CLIENT_STATE_CHANGED)
            }
            RtmpEvent::ClientDisconnectRequested { reason } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_CLIENT_DISCONNECT_REQUESTED,
                primary: Bytes::from(reason.into_bytes()),
                ..Self::empty(
                    RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_CLIENT_DISCONNECT_REQUESTED,
                )
            },
            RtmpEvent::Metadata { stream_id, values } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_METADATA,
                stream_id,
                primary: encode_amf_values(&values),
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_METADATA)
            },
            RtmpEvent::Notify {
                stream_id,
                name,
                values,
            } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_NOTIFY,
                stream_id,
                primary: Bytes::from(name.into_bytes()),
                secondary: encode_amf_values(&values),
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_NOTIFY)
            },
            RtmpEvent::MediaData {
                stream_id,
                timestamp_ms,
                media_type,
                payload,
            } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_MEDIA_DATA,
                stream_id,
                timestamp_ms,
                media_type: map_media_type(media_type),
                primary: payload,
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_MEDIA_DATA)
            },
            RtmpEvent::StreamClosed { stream_id } => Self {
                kind: RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_STREAM_CLOSED,
                stream_id,
                ..Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_STREAM_CLOSED)
            },
            RtmpEvent::PeerClosed => {
                Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_EVENT_PEER_CLOSED)
            }
            // The C ABI predates these player-control events. We surface them as `KIND_NONE`
            // so the FFI consumer doesn't crash on unknown discriminants. Adding dedicated
            // variants would be an ABI break and is left for a later major bump.
            RtmpEvent::SeekRequested { .. }
            | RtmpEvent::PauseRequested { .. }
            | RtmpEvent::ReceiveVideo { .. }
            | RtmpEvent::ReceiveAudio { .. } => {
                Self::empty(RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_NONE)
            }
        }
    }

    fn view(&self) -> Result<RtmpCoreOutputView, RtmpCoreApiError> {
        let primary_len = u32::try_from(self.primary.len())
            .map_err(|_| RtmpCoreApiError::RTMP_CORE_API_ERROR_OVERFLOW)?;
        let secondary_len = u32::try_from(self.secondary.len())
            .map_err(|_| RtmpCoreApiError::RTMP_CORE_API_ERROR_OVERFLOW)?;
        Ok(RtmpCoreOutputView {
            kind: self.kind,
            timer_id: self.timer_id,
            at_micros: self.at_micros,
            stream_id: self.stream_id,
            timestamp_ms: self.timestamp_ms,
            media_type: self.media_type,
            primary_ptr: self.primary.as_ptr(),
            primary_len,
            secondary_ptr: self.secondary.as_ptr(),
            secondary_len,
        })
    }
}

fn encode_amf_values(values: &[AmfValue]) -> Bytes {
    let mut payload = Vec::new();
    for value in values {
        value.encode(&mut payload);
    }
    Bytes::from(payload)
}

/// Handle to a `RTMP Core` resource.
/// `RTMP Core` 资源的句柄。
pub struct RtmpCoreHandle {
    core: RtmpCore,
    output_queue: VecDeque<OwnedOutput>,
    current_output: Option<OwnedOutput>,
    last_error_string: Option<CString>,
}

impl RtmpCoreHandle {
    fn new() -> Self {
        Self {
            core: RtmpCore::new(),
            output_queue: VecDeque::new(),
            current_output: None,
            last_error_string: None,
        }
    }

    fn clear_last_error(&mut self) {
        self.last_error_string = None;
    }

    fn set_last_error(&mut self, message: impl AsRef<str>) {
        self.last_error_string = CString::new(message.as_ref())
            .ok()
            .or_else(|| CString::new("ffi error message contains NUL byte").ok());
    }

    fn apply_input(&mut self, input: CoreInput) -> RtmpCoreApiError {
        self.clear_last_error();
        match self.core.handle_input(input) {
            Ok(outputs) => {
                self.output_queue
                    .extend(outputs.into_iter().map(OwnedOutput::from_core));
                RtmpCoreApiError::RTMP_CORE_API_ERROR_OK
            }
            Err(error) => {
                self.set_last_error(error.to_string());
                RtmpCoreApiError::RTMP_CORE_API_ERROR_CORE
            }
        }
    }

    fn pending_output_count(&self) -> u32 {
        u32::try_from(self.output_queue.len()).unwrap_or(u32::MAX)
    }
}

fn map_media_type(media_type: RtmpMediaType) -> RtmpCoreOutputMediaType {
    match media_type {
        RtmpMediaType::Audio => RtmpCoreOutputMediaType::RTMP_CORE_OUTPUT_MEDIA_TYPE_AUDIO,
        RtmpMediaType::Video => RtmpCoreOutputMediaType::RTMP_CORE_OUTPUT_MEDIA_TYPE_VIDEO,
        RtmpMediaType::Data => RtmpCoreOutputMediaType::RTMP_CORE_OUTPUT_MEDIA_TYPE_DATA,
    }
}

unsafe fn handle_mut<'a>(handle: *mut RtmpCoreHandle) -> Option<&'a mut RtmpCoreHandle> {
    if handle.is_null() {
        return None;
    }
    Some(unsafe { &mut *handle })
}

unsafe fn read_bytes<'a>(data: *const u8, len: u32) -> Result<&'a [u8], RtmpCoreApiError> {
    if data.is_null() {
        return if len == 0 {
            Ok(&[])
        } else {
            Err(RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER)
        };
    }
    Ok(unsafe { std::slice::from_raw_parts(data, len as usize) })
}

unsafe fn read_utf8<'a>(data: *const u8, len: u32) -> Result<&'a str, RtmpCoreApiError> {
    let bytes = unsafe { read_bytes(data, len)? };
    std::str::from_utf8(bytes).map_err(|_| RtmpCoreApiError::RTMP_CORE_API_ERROR_INVALID_ARGUMENT)
}

/// `rtmp_library_version` function.
/// `rtmp_library_version` 函数。
#[unsafe(no_mangle)]
pub extern "C" fn rtmp_library_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// `rtmp_core_new` function.
/// `rtmp_core_new` 函数。
#[unsafe(no_mangle)]
pub extern "C" fn rtmp_core_new() -> *mut RtmpCoreHandle {
    Box::into_raw(Box::new(RtmpCoreHandle::new()))
}

/// `rtmp_core_free` function.
/// `rtmp_core_free` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_free(handle: *mut RtmpCoreHandle) {
    if handle.is_null() {
        return;
    }
    let _ = unsafe { Box::from_raw(handle) };
}

/// `rtmp_core_get_last_error` function.
/// `rtmp_core_get_last_error` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_get_last_error(handle: *const RtmpCoreHandle) -> *const c_char {
    if handle.is_null() {
        return NULL_POINTER_ERROR.as_ptr().cast();
    }
    let handle_ref = unsafe { &*handle };
    handle_ref
        .last_error_string
        .as_ref()
        .map_or(EMPTY_ERROR.as_ptr().cast(), |text| text.as_ptr())
}

/// `rtmp_core_pending_output_count` function.
/// `rtmp_core_pending_output_count` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_pending_output_count(handle: *const RtmpCoreHandle) -> u32 {
    if handle.is_null() {
        return 0;
    }
    let handle_ref = unsafe { &*handle };
    handle_ref.pending_output_count()
}

/// `rtmp_core_clear_outputs` function.
/// `rtmp_core_clear_outputs` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_clear_outputs(handle: *mut RtmpCoreHandle) {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return;
    };
    handle_ref.current_output = None;
    handle_ref.output_queue.clear();
}

/// `rtmp_core_clear_output` function.
/// `rtmp_core_clear_output` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_clear_output(handle: *mut RtmpCoreHandle) {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return;
    };
    handle_ref.current_output = None;
}

/// `rtmp_core_next_output` function.
/// `rtmp_core_next_output` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_next_output(
    handle: *mut RtmpCoreHandle,
    output: *mut RtmpCoreOutputView,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    if output.is_null() {
        handle_ref.set_last_error("output view pointer is null");
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    }
    handle_ref.clear_last_error();
    let Some(next) = handle_ref.output_queue.pop_front() else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NO_OUTPUT;
    };
    handle_ref.current_output = Some(next);
    let Some(current) = handle_ref.current_output.as_ref() else {
        handle_ref.set_last_error("output cursor failure");
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_CORE;
    };
    let Ok(view) = current.view() else {
        handle_ref.set_last_error("output payload length overflow");
        handle_ref.current_output = None;
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_OVERFLOW;
    };
    unsafe {
        *output = view;
    }
    RtmpCoreApiError::RTMP_CORE_API_ERROR_OK
}

/// `rtmp_core_handle_bytes` function.
/// `rtmp_core_handle_bytes` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_handle_bytes(
    handle: *mut RtmpCoreHandle,
    data: *const u8,
    len: u32,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    let Ok(bytes) = (unsafe { read_bytes(data, len) }) else {
        handle_ref.set_last_error("input bytes pointer is null");
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    handle_ref.apply_input(CoreInput::Bytes(Bytes::copy_from_slice(bytes)))
}

/// `rtmp_core_handle_timeout` function.
/// `rtmp_core_handle_timeout` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_handle_timeout(
    handle: *mut RtmpCoreHandle,
    timer_id: u64,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    handle_ref.apply_input(CoreInput::Timeout { id: timer_id })
}

fn command_no_payload(
    handle_ref: &mut RtmpCoreHandle,
    command: RtmpCoreCommand,
) -> RtmpCoreApiError {
    handle_ref.apply_input(CoreInput::Command(command))
}

/// `rtmp_core_command_accept_publish` function.
/// `rtmp_core_command_accept_publish` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_command_accept_publish(
    handle: *mut RtmpCoreHandle,
    stream_id: u32,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    command_no_payload(handle_ref, RtmpCoreCommand::AcceptPublish { stream_id })
}

/// `rtmp_core_command_reject_publish` function.
/// `rtmp_core_command_reject_publish` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_command_reject_publish(
    handle: *mut RtmpCoreHandle,
    stream_id: u32,
    description_ptr: *const u8,
    description_len: u32,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    let Ok(description) = (unsafe { read_utf8(description_ptr, description_len) }) else {
        handle_ref.set_last_error("description is not valid UTF-8");
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_INVALID_ARGUMENT;
    };
    command_no_payload(
        handle_ref,
        RtmpCoreCommand::RejectPublish {
            stream_id,
            description: description.to_owned(),
        },
    )
}

/// `rtmp_core_command_accept_play` function.
/// `rtmp_core_command_accept_play` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_command_accept_play(
    handle: *mut RtmpCoreHandle,
    stream_id: u32,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    command_no_payload(handle_ref, RtmpCoreCommand::AcceptPlay { stream_id })
}

/// `rtmp_core_command_accept_play_configured` function.
/// `rtmp_core_command_accept_play_configured` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_command_accept_play_configured(
    handle: *mut RtmpCoreHandle,
    stream_id: u32,
    emit_play_status: bool,
    emit_sample_access: bool,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    command_no_payload(
        handle_ref,
        RtmpCoreCommand::AcceptPlayConfigured {
            stream_id,
            emit_play_status,
            emit_sample_access,
        },
    )
}

/// `rtmp_core_command_reject_play` function.
/// `rtmp_core_command_reject_play` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_command_reject_play(
    handle: *mut RtmpCoreHandle,
    stream_id: u32,
    description_ptr: *const u8,
    description_len: u32,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    let Ok(description) = (unsafe { read_utf8(description_ptr, description_len) }) else {
        handle_ref.set_last_error("description is not valid UTF-8");
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_INVALID_ARGUMENT;
    };
    command_no_payload(
        handle_ref,
        RtmpCoreCommand::RejectPlay {
            stream_id,
            description: description.to_owned(),
        },
    )
}

unsafe fn read_payload(
    handle_ref: &mut RtmpCoreHandle,
    payload_ptr: *const u8,
    payload_len: u32,
) -> Result<Bytes, RtmpCoreApiError> {
    let payload = unsafe { read_bytes(payload_ptr, payload_len) };
    match payload {
        Ok(bytes) => Ok(Bytes::copy_from_slice(bytes)),
        Err(error) => {
            handle_ref.set_last_error("payload pointer is null");
            Err(error)
        }
    }
}

/// `rtmp_core_command_send_metadata` function.
/// `rtmp_core_command_send_metadata` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_command_send_metadata(
    handle: *mut RtmpCoreHandle,
    stream_id: u32,
    timestamp_ms: u32,
    payload_ptr: *const u8,
    payload_len: u32,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    let Ok(payload) = (unsafe { read_payload(handle_ref, payload_ptr, payload_len) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    command_no_payload(
        handle_ref,
        RtmpCoreCommand::SendMetadata {
            stream_id,
            timestamp_ms,
            payload,
        },
    )
}

/// `rtmp_core_command_send_audio` function.
/// `rtmp_core_command_send_audio` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_command_send_audio(
    handle: *mut RtmpCoreHandle,
    stream_id: u32,
    timestamp_ms: u32,
    payload_ptr: *const u8,
    payload_len: u32,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    let Ok(payload) = (unsafe { read_payload(handle_ref, payload_ptr, payload_len) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    command_no_payload(
        handle_ref,
        RtmpCoreCommand::SendAudio {
            stream_id,
            timestamp_ms,
            payload,
        },
    )
}

/// `rtmp_core_command_send_video` function.
/// `rtmp_core_command_send_video` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_command_send_video(
    handle: *mut RtmpCoreHandle,
    stream_id: u32,
    timestamp_ms: u32,
    payload_ptr: *const u8,
    payload_len: u32,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    let Ok(payload) = (unsafe { read_payload(handle_ref, payload_ptr, payload_len) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    command_no_payload(
        handle_ref,
        RtmpCoreCommand::SendVideo {
            stream_id,
            timestamp_ms,
            payload,
        },
    )
}

/// `rtmp_core_command_send_notify` function.
/// `rtmp_core_command_send_notify` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_command_send_notify(
    handle: *mut RtmpCoreHandle,
    stream_id: u32,
    timestamp_ms: u32,
    payload_ptr: *const u8,
    payload_len: u32,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    let Ok(payload) = (unsafe { read_payload(handle_ref, payload_ptr, payload_len) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    command_no_payload(
        handle_ref,
        RtmpCoreCommand::SendNotify {
            stream_id,
            timestamp_ms,
            payload,
        },
    )
}

/// `rtmp_core_command_close_stream` function.
/// `rtmp_core_command_close_stream` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_command_close_stream(
    handle: *mut RtmpCoreHandle,
    stream_id: u32,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    command_no_payload(handle_ref, RtmpCoreCommand::CloseStream { stream_id })
}

/// `rtmp_core_command_close_connection` function.
/// `rtmp_core_command_close_connection` 函数。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rtmp_core_command_close_connection(
    handle: *mut RtmpCoreHandle,
) -> RtmpCoreApiError {
    let Some(handle_ref) = (unsafe { handle_mut(handle) }) else {
        return RtmpCoreApiError::RTMP_CORE_API_ERROR_NULL_POINTER;
    };
    command_no_payload(handle_ref, RtmpCoreCommand::CloseConnection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_bytes_exposes_write_output() {
        let handle = rtmp_core_new();
        let mut c0c1 = vec![0u8; 1537];
        c0c1[0] = 3;
        let ret = unsafe { rtmp_core_handle_bytes(handle, c0c1.as_ptr(), c0c1.len() as u32) };
        assert_eq!(ret, RtmpCoreApiError::RTMP_CORE_API_ERROR_OK);
        assert!(unsafe { rtmp_core_pending_output_count(handle) } >= 1);

        let mut output = RtmpCoreOutputView::default();
        let ret = unsafe { rtmp_core_next_output(handle, &mut output) };
        assert_eq!(ret, RtmpCoreApiError::RTMP_CORE_API_ERROR_OK);
        assert_eq!(output.kind, RtmpCoreOutputKind::RTMP_CORE_OUTPUT_KIND_WRITE);
        assert!(output.primary_len >= 1537);

        unsafe { rtmp_core_free(handle) };
    }

    #[test]
    fn next_output_returns_no_output_when_empty() {
        let handle = rtmp_core_new();
        let mut output = RtmpCoreOutputView::default();
        let ret = unsafe { rtmp_core_next_output(handle, &mut output) };
        assert_eq!(ret, RtmpCoreApiError::RTMP_CORE_API_ERROR_NO_OUTPUT);
        unsafe { rtmp_core_free(handle) };
    }
}
