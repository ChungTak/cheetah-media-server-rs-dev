use crate::chunk::RtmpChunkDecoder;

use super::CoreOutput;

mod capture;
mod command;
mod handshake;
mod media;

pub(super) fn decode_first_message(
    decoder: &mut RtmpChunkDecoder,
    outputs: &[CoreOutput],
) -> crate::chunk::RtmpChunk {
    let wire = outputs
        .iter()
        .find_map(|output| match output {
            CoreOutput::Write(bytes) => Some(bytes.clone()),
            _ => None,
        })
        .expect("expected Write output");

    let mut pending = wire.to_vec();
    loop {
        let (consumed, maybe_chunk) = decoder.decode(&pending).expect("decode chunk wire");
        pending.drain(..consumed);
        if let Some(chunk) = maybe_chunk {
            return chunk;
        }
    }
}
