//! Pending-candidate / pending-message buffers.
//!
//! Phase 05 §5.6 spells out the ordering rules:
//!
//! 1. Trickle ICE candidates may arrive *before* the remote SDP answer.
//!    They must be buffered until the answer lands and then flushed
//!    in arrival order.
//! 2. Repeat candidates from a quirky peer must not crash the client.
//!    The buffer dedupes by full candidate string.
//! 3. Buffers are bounded so a misbehaving peer can't grow memory.
//!    On overflow the oldest candidate is evicted and the eviction is
//!    surfaced to the caller (which logs a diagnostic).

use std::collections::{HashSet, VecDeque};

use thiserror::Error;

/// Default cap on pending candidates per session.
pub const PENDING_CANDIDATE_DEFAULT_CAP: usize = 32;

/// Result of [`PendingCandidateBuffer::push`]. Callers translate
/// [`PushOutcome::Evicted`] into a diagnostic event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    /// Buffered; ready to flush.
    Buffered,
    /// Already buffered (deduped).
    Duplicate,
    /// Buffered but the oldest entry was dropped to make room.
    Evicted { dropped: PendingCandidate },
    /// Buffer is in the closed state and refused the push.
    Closed,
}

/// Buffer states. Mirrors the Phase 05 architecture diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferState {
    /// SDP answer has not arrived yet — pushes accumulate.
    AwaitingAnswer,
    /// Answer applied — buffer is drained on every push.
    Open,
    /// Session was cancelled / the buffer was finalized.
    Closed,
}

/// One buffered remote ICE candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCandidate {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u32>,
}

/// Bounded buffer for pending candidates. Constructed in
/// [`BufferState::AwaitingAnswer`]; transitions to `Open` once the
/// answer is applied (callers should drain the buffer at the same
/// time). Closing is final.
#[derive(Debug)]
pub struct PendingCandidateBuffer {
    queue: VecDeque<PendingCandidate>,
    seen: HashSet<String>,
    state: BufferState,
    cap: usize,
}

/// Error returned by `Pending Buffer` operations.
/// `Pending Buffer` 操作返回的错误。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum PendingBufferError {
    #[error("buffer cap must be > 0")]
    InvalidCap,
}

impl PendingCandidateBuffer {
    /// Creates a new `PendingCandidateBuffer` instance.
    /// 创建新的 `PendingCandidateBuffer` 实例。
    pub fn new(cap: usize) -> Result<Self, PendingBufferError> {
        if cap == 0 {
            return Err(PendingBufferError::InvalidCap);
        }
        Ok(Self {
            queue: VecDeque::with_capacity(cap),
            seen: HashSet::new(),
            state: BufferState::AwaitingAnswer,
            cap,
        })
    }

    /// `state` function of `PendingCandidateBuffer`.
    /// `PendingCandidateBuffer` 的 `state` 函数。
    pub fn state(&self) -> BufferState {
        self.state
    }

    /// Returns the number of elements.
    /// 返回元素数量。
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Returns true when there are no elements.
    /// 没有元素时返回 true。
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Push a candidate. The semantics depend on [`state`](Self::state):
    /// before the answer arrives candidates accumulate; after the
    /// answer they pass straight through (the caller still needs to
    /// drain on every push or call [`drain`](Self::drain)).
    pub fn push(&mut self, candidate: PendingCandidate) -> PushOutcome {
        match self.state {
            BufferState::Closed => return PushOutcome::Closed,
            BufferState::AwaitingAnswer | BufferState::Open => {}
        }
        if !self.seen.insert(candidate.candidate.clone()) {
            return PushOutcome::Duplicate;
        }
        let evicted = if self.queue.len() >= self.cap {
            self.queue.pop_front()
        } else {
            None
        };
        if let Some(ref ev) = evicted {
            self.seen.remove(&ev.candidate);
        }
        self.queue.push_back(candidate);
        match evicted {
            Some(dropped) => PushOutcome::Evicted { dropped },
            None => PushOutcome::Buffered,
        }
    }

    /// Mark the answer as applied and return the buffered candidates
    /// in arrival order. Subsequent pushes that pre-date a `close()`
    /// will pass through but still get deduped.
    pub fn flush(&mut self) -> Vec<PendingCandidate> {
        if matches!(self.state, BufferState::Closed) {
            return Vec::new();
        }
        self.state = BufferState::Open;
        self.queue.drain(..).collect()
    }

    /// Drain whatever is currently buffered without changing state.
    /// Useful when the caller wants to force-flush before close.
    pub fn drain(&mut self) -> Vec<PendingCandidate> {
        let drained: Vec<PendingCandidate> = self.queue.drain(..).collect();
        self.seen.clear();
        for c in &drained {
            self.seen.insert(c.candidate.clone());
        }
        drained
    }

    /// Close the buffer. Future pushes return [`PushOutcome::Closed`].
    pub fn close(&mut self) {
        self.state = BufferState::Closed;
        self.queue.clear();
        self.seen.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(s: &str) -> PendingCandidate {
        PendingCandidate {
            candidate: s.to_string(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        }
    }

    #[test]
    fn rejects_zero_cap() {
        let err = PendingCandidateBuffer::new(0).unwrap_err();
        assert_eq!(err, PendingBufferError::InvalidCap);
    }

    #[test]
    fn buffers_until_flush_then_returns_in_order() {
        let mut buf = PendingCandidateBuffer::new(4).unwrap();
        assert_eq!(buf.push(cand("a")), PushOutcome::Buffered);
        assert_eq!(buf.push(cand("b")), PushOutcome::Buffered);
        assert_eq!(buf.state(), BufferState::AwaitingAnswer);
        let flushed = buf.flush();
        assert_eq!(flushed.len(), 2);
        assert_eq!(flushed[0].candidate, "a");
        assert_eq!(flushed[1].candidate, "b");
        assert_eq!(buf.state(), BufferState::Open);
    }

    #[test]
    fn duplicates_are_deduped() {
        let mut buf = PendingCandidateBuffer::new(4).unwrap();
        assert_eq!(buf.push(cand("a")), PushOutcome::Buffered);
        assert_eq!(buf.push(cand("a")), PushOutcome::Duplicate);
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn overflow_evicts_oldest() {
        let mut buf = PendingCandidateBuffer::new(2).unwrap();
        buf.push(cand("a"));
        buf.push(cand("b"));
        let outcome = buf.push(cand("c"));
        match outcome {
            PushOutcome::Evicted { dropped } => assert_eq!(dropped.candidate, "a"),
            other => panic!("expected eviction, got {other:?}"),
        }
        let flushed = buf.flush();
        assert_eq!(
            flushed
                .iter()
                .map(|c| c.candidate.clone())
                .collect::<Vec<_>>(),
            vec!["b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn close_makes_subsequent_pushes_no_op() {
        let mut buf = PendingCandidateBuffer::new(4).unwrap();
        buf.push(cand("a"));
        buf.close();
        assert_eq!(buf.state(), BufferState::Closed);
        assert_eq!(buf.push(cand("b")), PushOutcome::Closed);
        assert!(buf.is_empty());
    }

    #[test]
    fn open_state_still_dedupes() {
        let mut buf = PendingCandidateBuffer::new(4).unwrap();
        buf.flush(); // -> Open
        assert_eq!(buf.push(cand("x")), PushOutcome::Buffered);
        assert_eq!(buf.push(cand("x")), PushOutcome::Duplicate);
    }
}
