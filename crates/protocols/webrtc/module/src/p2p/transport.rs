//! P2P signaling transport abstraction.
//!
//! The transport sits between the wire schema in [`super::message`] and
//! whatever physical channel the keeper is bound to. Production code
//! will plug a `tokio-tungstenite` WebSocket here; tests and the
//! signaling state machine in [`super::client`] use an in-memory
//! transport so the state machine can be exercised without a real
//! network.
//!
//! The trait is deliberately minimal:
//!
//! * `send` enqueues an outbound message.
//! * `recv` waits for the next inbound message or transport close.
//! * `close` releases resources.
//!
//! Runtime-neutral: the trait is `async_trait`-flavoured but does not
//! reach into `tokio::*` types. Implementations decide how to bridge
//! to their runtime.

use async_trait::async_trait;
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use thiserror::Error;

use super::message::P2pMessage;

/// Failures the transport can surface.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum P2pTransportError {
    /// `Closed` variant.
    /// `Closed` 变体.
    #[error("transport closed")]
    Closed,
    /// `Io` variant.
    /// `Io` 变体.
    #[error("transport error: {0}")]
    Io(String),
    /// `Encode` variant.
    /// `Encode` 变体.
    #[error("encoding failed: {0}")]
    Encode(String),
    /// `Decode` variant.
    /// `Decode` 变体.
    #[error("decoding failed: {0}")]
    Decode(String),
}

/// Outcome of a `recv` call.
#[derive(Debug, Clone, PartialEq)]
pub enum P2pTransportEvent {
    /// A signaling message arrived.
    Message(P2pMessage),
    /// The transport was closed by the peer or by us.
    Closed,
    /// A protocol error happened mid-stream. The keeper interprets
    /// this as a reconnect trigger.
    Error(String),
}

/// Pure-async transport abstraction. See module-level docs for the
/// rationale.
#[async_trait]
pub trait P2pTransport: Send + Sync {
    async fn send(&self, message: P2pMessage) -> Result<(), P2pTransportError>;
    async fn recv(&self) -> Result<P2pTransportEvent, P2pTransportError>;
    async fn close(&self);
}

/// In-memory paired transport for tests / state-machine drivers. A
/// pair of [`InMemoryTransport`] instances are wired so messages sent
/// on one side surface on the other side's `recv`.
#[derive(Debug)]
pub struct InMemoryTransport {
    /// `inbound` field.
    /// `inbound` 字段.
    inbound: Arc<Mutex<mpsc::Receiver<P2pTransportEvent>>>,
    /// `outbound` field.
    /// `outbound` 字段.
    outbound: mpsc::Sender<P2pTransportEvent>,
    /// Mirror of the *peer's* outbound. Used by tests to inspect what
    /// the local end emitted, without blocking on `recv`.
    pub recorder: Arc<Mutex<Vec<P2pMessage>>>,
    /// Closed flag.
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl InMemoryTransport {
    /// Create a connected pair. `(local, remote)` — anything `local`
    /// sends becomes a message that `remote.recv()` returns and vice
    /// versa.
    pub fn pair(capacity: usize) -> (Self, Self) {
        let (a_tx, a_rx) = mpsc::channel(capacity.max(4));
        let (b_tx, b_rx) = mpsc::channel(capacity.max(4));
        let recorder_a = Arc::new(Mutex::new(Vec::new()));
        let recorder_b = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let local = Self {
            inbound: Arc::new(Mutex::new(b_rx)),
            outbound: a_tx,
            recorder: recorder_a,
            closed: closed.clone(),
        };
        let remote = Self {
            inbound: Arc::new(Mutex::new(a_rx)),
            outbound: b_tx,
            recorder: recorder_b,
            closed,
        };
        (local, remote)
    }
}

#[async_trait]
impl P2pTransport for InMemoryTransport {
    async fn send(&self, message: P2pMessage) -> Result<(), P2pTransportError> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(P2pTransportError::Closed);
        }
        self.recorder.lock().await.push(message.clone());
        self.outbound
            .clone()
            .send(P2pTransportEvent::Message(message))
            .await
            .map_err(|_| P2pTransportError::Closed)
    }

    async fn recv(&self) -> Result<P2pTransportEvent, P2pTransportError> {
        let mut guard = self.inbound.lock().await;
        match guard.next().await {
            Some(event) => Ok(event),
            None => Ok(P2pTransportEvent::Closed),
        }
    }

    async fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        // Best-effort: notify the peer.
        let _ = self.outbound.clone().send(P2pTransportEvent::Closed).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::message::{P2pMessage, P2pMessageHeader};

    #[tokio::test(flavor = "current_thread")]
    async fn pair_round_trip_delivers_messages_both_ways() {
        let (a, b) = InMemoryTransport::pair(4);
        a.send(P2pMessage::Ping {
            header: P2pMessageHeader::default(),
        })
        .await
        .unwrap();
        let received = b.recv().await.unwrap();
        match received {
            P2pTransportEvent::Message(P2pMessage::Ping { .. }) => {}
            other => panic!("unexpected: {other:?}"),
        }

        b.send(P2pMessage::Pong {
            header: P2pMessageHeader::default(),
        })
        .await
        .unwrap();
        let received = a.recv().await.unwrap();
        match received {
            P2pTransportEvent::Message(P2pMessage::Pong { .. }) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn close_propagates_and_blocks_send() {
        let (a, b) = InMemoryTransport::pair(4);
        a.close().await;
        match b.recv().await.unwrap() {
            P2pTransportEvent::Closed => {}
            other => panic!("expected Closed, got {other:?}"),
        }
        let err = a
            .send(P2pMessage::Ping {
                header: P2pMessageHeader::default(),
            })
            .await
            .unwrap_err();
        assert_eq!(err, P2pTransportError::Closed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn recorder_captures_outbound_messages() {
        let (a, _b) = InMemoryTransport::pair(4);
        a.send(P2pMessage::Ping {
            header: P2pMessageHeader::default(),
        })
        .await
        .unwrap();
        assert_eq!(a.recorder.lock().await.len(), 1);
    }
}
