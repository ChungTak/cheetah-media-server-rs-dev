#![no_main]

//! Fuzz target for the WebRTC-over-TCP RFC 4571 framing decoder.
//!
//! Phase 05 contract: `Tcp4571Decoder::next_frame` must never panic
//! regardless of input bytes. Errors (oversize advertised length)
//! return `Tcp4571Error::FrameTooLarge`; partial frames return
//! `Ok(None)`. We exercise every possible split point of the input
//! by feeding the decoder one byte at a time, then drain to verify
//! the streaming path never gets stuck.

use cheetah_webrtc_driver_tokio::Tcp4571Decoder;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut dec = Tcp4571Decoder::new();
    for byte in data {
        dec.extend(std::slice::from_ref(byte));
        // Drain any complete frames whenever they become ready.
        loop {
            match dec.next_frame() {
                Ok(Some(_frame)) => continue,
                Ok(None) => break,
                Err(_) => return,
            }
        }
    }
    // After feeding all bytes, residual `next_frame` must still not
    // panic and must return one of the structured outcomes.
    let _ = dec.next_frame();
});
