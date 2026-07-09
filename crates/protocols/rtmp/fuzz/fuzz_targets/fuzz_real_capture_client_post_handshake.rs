//! Feed post-handshake server responses derived from real C2S capture replay
//! through the Sans-I/O client core.

#![no_main]

mod common;

use bytes::Bytes;
use cheetah_rtmp_core::{CoreInput, RtmpCore, RtmpCoreCommand};
use common::{
    build_transport_view, capture_records_from_data_or_seed, derive_server_post_handshake_writes,
    CaptureViewKind,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (_case_name, records) = capture_records_from_data_or_seed(data);
    if records.is_empty() {
        return;
    }
    let response_view_selector = data.get(1).copied().unwrap_or_default();
    let response_chunk_hint = data.get(2).copied().unwrap_or_default();
    let server_writes =
        derive_server_post_handshake_writes(&records, response_view_selector, response_chunk_hint);
    if server_writes.is_empty() {
        return;
    }

    let mut core = RtmpCore::new_client();
    prime_client_state(&mut core, data);

    let write_refs: Vec<&[u8]> = server_writes.iter().map(Bytes::as_ref).collect();
    let client_mode = CaptureViewKind::from_selector(data.get(3).copied().unwrap_or_default());
    let client_parameter = data.get(4).copied().unwrap_or_default();
    for chunk in build_transport_view(&write_refs, client_mode, client_parameter) {
        if core.handle_input(CoreInput::Bytes(chunk)).is_err() {
            break;
        }
    }
});

fn prime_client_state(core: &mut RtmpCore, data: &[u8]) {
    let _ = core.handle_input(CoreInput::Command(RtmpCoreCommand::ClientConnect {
        app: "live".to_string(),
        flash_ver: "FMLE/3.0".to_string(),
        tc_url: "rtmp://127.0.0.1/live".to_string(),
    }));
    let _ = core.handle_input(CoreInput::Command(RtmpCoreCommand::ClientCreateStream {
        transaction_id: 2.0,
    }));

    if data.get(5).copied().unwrap_or_default() & 1 == 0 {
        let _ = core.handle_input(CoreInput::Command(RtmpCoreCommand::ClientPublish {
            stream_id: 1,
            transaction_id: 3.0,
            stream_name: "capture".to_string(),
        }));
    } else {
        let _ = core.handle_input(CoreInput::Command(RtmpCoreCommand::ClientPlay {
            stream_id: 1,
            transaction_id: 3.0,
            stream_name: "capture".to_string(),
            start: -2.0,
        }));
    }
}
