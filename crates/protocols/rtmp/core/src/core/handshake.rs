use alloc::string::ToString;
use alloc::vec::Vec;

use bytes::Bytes;

use super::{CoreOutput, HandshakeRole, HandshakeState, RtmpCore, RtmpCoreError};

impl RtmpCore {
    /// `try_handshake` function.
    /// `try_handshake` 函数.
    pub(super) fn try_handshake(
        &mut self,
        bytes: Bytes,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let HandshakeRole::Server(handshake) = &mut self.handshake else {
            self.state = HandshakeState::Closed;
            return Err(RtmpCoreError::Handshake(
                "client core does not accept raw bytes during handshake".to_string(),
            ));
        };

        if self.state == HandshakeState::Handshaking
            && bytes.first().is_some_and(|version| *version != 3)
        {
            let version = bytes[0];
            self.state = HandshakeState::Closed;
            return Err(RtmpCoreError::InvalidHandshakeVersion(version));
        }

        handshake.feed_recv_buf(&bytes).map_err(|error| {
            self.state = HandshakeState::Closed;
            RtmpCoreError::Handshake(error.reason)
        })?;

        if !handshake.send_buf().is_empty() {
            let pending = Bytes::copy_from_slice(handshake.send_buf());
            handshake.advance_send_buf(pending.len());
            out.push(CoreOutput::Write(pending));
            if self.state == HandshakeState::Handshaking {
                self.state = HandshakeState::WaitC2;
            }
        }

        let recv_complete = handshake.is_recv_complete();
        if recv_complete {
            let remaining = {
                let HandshakeRole::Server(handshake) = &mut self.handshake else {
                    unreachable!();
                };
                handshake.take_recv_buf()
            };
            self.state = HandshakeState::Ready;
            self.send_set_chunk_size(self.out_chunk_size as u32, out)?;
            self.send_window_ack_size(out)?;
            self.send_set_peer_bandwidth(out)?;

            if !remaining.is_empty() {
                self.process_ready_bytes(Bytes::from(remaining), out)?;
            }
        }

        Ok(())
    }
}
