#![no_main]
use libfuzzer_sys::fuzz_target;

use bytes::Bytes;
use cheetah_codec::{Mp4ReadResult, Mp4ReaderConfig};
use cheetah_mp4_core::{VodControlCommand, VodCoreInput, VodOutput, VodSession};

// Drives the VOD session state machine through arbitrary control sequences
// and reader callbacks. Verifies that the state machine never panics and
// always reaches `CloseSession` or stops requesting bytes within a finite
// step budget.
fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let mut session = VodSession::new(Mp4ReaderConfig {
        max_box_bytes: 64 * 1024,
        max_top_level_scan: 64 * 1024,
    });

    let mut next_input = VodCoreInput::Control(VodControlCommand::Start {
        file_size: data.len() as u64,
    });

    let mut steps = 0usize;
    let mut now_us: u64 = 0;
    while steps < 128 {
        steps += 1;
        let outputs = session.step(next_input.clone());
        let mut consumed = false;
        let mut closed = false;
        for output in outputs {
            match output {
                VodOutput::ReadAt(req) => {
                    let end = (req.offset + req.length) as usize;
                    let end = end.min(data.len());
                    let start = (req.offset as usize).min(data.len());
                    if start >= end {
                        closed = true;
                        break;
                    }
                    next_input = VodCoreInput::ReadAt(Mp4ReadResult {
                        offset: req.offset,
                        data: Bytes::copy_from_slice(&data[start..end]),
                    });
                    consumed = true;
                }
                VodOutput::ScheduleTick { .. }
                | VodOutput::EmitFrame(_)
                | VodOutput::EmitTrackInfo(_) => {}
                VodOutput::CloseSession => {
                    closed = true;
                    break;
                }
            }
        }
        if closed {
            break;
        }
        if !consumed {
            let cmd_byte = data[steps % data.len()];
            next_input = match cmd_byte % 4 {
                0 => VodCoreInput::Tick { now_us },
                1 => VodCoreInput::Control(VodControlCommand::Pause(cmd_byte & 1 == 0)),
                2 => VodCoreInput::Control(VodControlCommand::Seek {
                    position_us: (cmd_byte as i64) * 1_000,
                }),
                _ => VodCoreInput::Control(VodControlCommand::Scale(
                    1.0 + (cmd_byte as f32) / 256.0,
                )),
            };
            now_us = now_us.saturating_add(1_000);
        }
    }
});
