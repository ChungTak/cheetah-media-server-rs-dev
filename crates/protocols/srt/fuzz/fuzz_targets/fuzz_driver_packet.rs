#![no_main]

use libfuzzer_sys::fuzz_target;
use shiguredo_srt::{ConnectionOptions, SrtConnection, Timestamp};

fuzz_target!(|data: &[u8]| {
    let mut connection = SrtConnection::new_listener(ConnectionOptions {
        socket_id: 0xC000_0001,
        initial_seq: Some(1),
        syn_cookie: Some(0x5A17_0001),
        ..Default::default()
    });

    let _ = connection.feed_recv_buf(data, Timestamp::from_micros(0));

    for _ in 0..64 {
        if connection.poll_event().is_none() {
            break;
        }
    }

    for _ in 0..64 {
        if connection.poll_output().is_none() {
            break;
        }
    }
});

