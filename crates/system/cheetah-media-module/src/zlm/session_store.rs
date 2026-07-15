//! In-memory session and login rate-limit state for the ZLM adapter.
//!
//! Session state and failed-login counters are kept inside the adapter module;
//! they are not passed into the domain layer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use cheetah_media_api::{MediaScope, Principal};

const DEFAULT_MAX_SESSIONS: usize = 10_000;
const MAX_FAILED_ATTEMPTS: usize = 5;
const MAX_FAILED_USERNAMES: usize = 10_000;
const FAILED_ATTEMPT_WINDOW_SECS: u64 = 15 * 60;

pub(crate) struct SessionEntry {
    pub principal: Principal,
    pub expires_at: Instant,
}

/// In-memory store for authenticated sessions and per-username failed-login
/// timestamps.
pub(crate) struct SessionStore {
    max_sessions: AtomicUsize,
    sessions: RwLock<HashMap<String, SessionEntry>>,
    failed_attempts: RwLock<HashMap<String, Vec<Instant>>>,
}

impl SessionStore {
    pub(crate) fn new() -> Self {
        Self {
            max_sessions: AtomicUsize::new(DEFAULT_MAX_SESSIONS),
            sessions: RwLock::new(HashMap::new()),
            failed_attempts: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn set_max_sessions(&self, max: Option<usize>) {
        let value = max.map(|m| m.max(1)).unwrap_or(DEFAULT_MAX_SESSIONS);
        self.max_sessions.store(value, Ordering::Relaxed);
    }

    /// Validate a session token and return the associated principal if it is
    /// still valid. Expired entries are removed on lookup.
    pub(crate) fn validate(&self, token: &str, now: Instant) -> Option<Principal> {
        let mut sessions = self.sessions.write().unwrap();
        let entry = sessions.get(token)?;
        if now > entry.expires_at {
            sessions.remove(token);
            return None;
        }
        Some(entry.principal.clone())
    }

    /// Insert a new session. If the store is at capacity, expired sessions are
    /// removed first; if still at capacity, the oldest session is evicted.
    pub(crate) fn insert(&self, token: String, principal: Principal, ttl: Duration) {
        let now = Instant::now();
        let expires_at = now + ttl;

        let max_sessions = self.max_sessions.load(Ordering::Relaxed);
        let mut sessions = self.sessions.write().unwrap();
        if sessions.len() >= max_sessions {
            // Evict expired sessions.
            sessions.retain(|_, e| now <= e.expires_at);
        }
        if sessions.len() >= max_sessions {
            // Evict the oldest session by expiration time.
            if let Some(oldest) = sessions
                .iter()
                .min_by(|a, b| a.1.expires_at.cmp(&b.1.expires_at))
                .map(|(k, _)| k.clone())
            {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(
            token,
            SessionEntry {
                principal,
                expires_at,
            },
        );
    }

    /// Remove a session (logout).
    pub(crate) fn remove(&self, token: &str) {
        self.sessions.write().unwrap().remove(token);
    }

    /// Check whether a username is currently rate limited, without recording a
    /// new attempt.
    pub(crate) fn is_rate_limited(&self, username: &str) -> bool {
        let window = Duration::from_secs(FAILED_ATTEMPT_WINDOW_SECS);
        let now = Instant::now();
        let attempts = self.failed_attempts.read().unwrap();
        if let Some(entry) = attempts.get(username) {
            let recent = entry
                .iter()
                .filter(|t| now.duration_since(**t) < window)
                .count();
            recent >= MAX_FAILED_ATTEMPTS
        } else {
            false
        }
    }

    /// Record a failed login attempt and return `true` if the username is now
    /// rate limited.
    pub(crate) fn record_failed_attempt(&self, username: &str) -> bool {
        let window = Duration::from_secs(FAILED_ATTEMPT_WINDOW_SECS);
        let now = Instant::now();
        let mut attempts = self.failed_attempts.write().unwrap();
        // Keep the username map bounded: drop expired/stale timestamps and remove
        // entries that become empty, then refuse to create new keys once the
        // distinct-username cap is reached.
        attempts.retain(|_, v| {
            v.retain(|t| now.duration_since(*t) < window);
            !v.is_empty()
        });
        if attempts.len() >= MAX_FAILED_USERNAMES && !attempts.contains_key(username) {
            return false;
        }
        let entry = attempts.entry(username.to_string()).or_default();
        entry.retain(|t| now.duration_since(*t) < window);
        entry.push(now);
        entry.len() >= MAX_FAILED_ATTEMPTS
    }

    /// Clear failed login attempts for a username after a successful login.
    pub(crate) fn clear_failed_logins(&self, username: &str) {
        self.failed_attempts.write().unwrap().remove(username);
    }

    /// Convenience to build the full-scope principal used for a ZLM session.
    pub(crate) fn admin_principal(identity: String) -> Principal {
        Principal {
            identity,
            scopes: vec![
                MediaScope::MediaRead,
                MediaScope::MediaControl,
                MediaScope::MediaPublish,
                MediaScope::MediaConsume,
                MediaScope::RecordManage,
                MediaScope::FileRead,
                MediaScope::FileDelete,
                MediaScope::ServerAdmin,
            ],
            resource_grants: Vec::new(),
        }
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}
