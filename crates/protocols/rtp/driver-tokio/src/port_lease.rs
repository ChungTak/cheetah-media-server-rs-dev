//! Per-session UDP socket lease manager.
//!
//! `PortManager` centralises the per-session UDP socket maps so that a failed
//! `UdpSocket::bind` never leaves a partially-registered socket, and so that a
//! socket's reference count is released automatically if the caller decides the
//! bind should not be committed (e.g. the core rejected the `CreateServer`).
//!
//! 每会话 UDP 套接字租约管理器。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error};

use crate::load_limiter::LoadLimiter;
use crate::spawn_udp_reader;
use crate::PortRange;

#[derive(Clone)]
pub(crate) struct PortManager {
    inner: Arc<PortManagerInner>,
}

struct PortManagerInner {
    /// Active per-session UDP sockets keyed by their actual bound address.
    sockets: Mutex<HashMap<SocketAddr, Arc<UdpSocket>>>,
    /// Reference count of sessions sharing each socket.
    counts: Mutex<HashMap<SocketAddr, usize>>,
    /// Cancellation tokens for per-socket UDP reader tasks.
    cancels: Mutex<HashMap<SocketAddr, CancellationToken>>,
    /// Channel into the main driver loop for incoming UDP datagrams.
    udp_tx: mpsc::Sender<crate::RtpDatagram>,
    /// Optional channel for RTCP datagrams when RTP/RTCP muxing is enabled.
    rtcp_tx: Option<mpsc::Sender<crate::RtpDatagram>>,
    /// Whether this socket is RTP/RTCP muxed.
    rtcp_mux: bool,
    /// Size of the UDP read buffer.
    read_buffer_size: usize,
    /// Runtime used to timestamp incoming datagrams.
    runtime: Arc<dyn RuntimeApi>,
    /// Driver-wide cancellation token.
    cancel: CancellationToken,
    /// Shared driver load limiter.
    load_limiter: LoadLimiter,
    /// Optional bounded UDP port pool. Used when `addr.port() == 0`.
    udp_port_pool: Option<PortRange>,
    /// Next port to try in the pool, to spread allocations across the range.
    next_port: AtomicU16,
}

impl PortManager {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        udp_tx: mpsc::Sender<crate::RtpDatagram>,
        rtcp_tx: Option<mpsc::Sender<crate::RtpDatagram>>,
        rtcp_mux: bool,
        read_buffer_size: usize,
        runtime: Arc<dyn RuntimeApi>,
        cancel: CancellationToken,
        load_limiter: LoadLimiter,
        udp_port_pool: Option<PortRange>,
    ) -> Self {
        let next_port = udp_port_pool.as_ref().map_or(0, |p| p.start);
        Self {
            inner: Arc::new(PortManagerInner {
                sockets: Mutex::new(HashMap::new()),
                counts: Mutex::new(HashMap::new()),
                cancels: Mutex::new(HashMap::new()),
                udp_tx,
                rtcp_tx,
                rtcp_mux,
                read_buffer_size,
                runtime,
                cancel,
                load_limiter,
                udp_port_pool,
                next_port: AtomicU16::new(next_port),
            }),
        }
    }

    /// Acquire a UDP socket lease for `addr`.
    ///
    /// If `reuse` is `true` and an existing socket is already bound to `addr`,
    /// its reference count is incremented and a lease sharing that socket is
    /// returned. Otherwise a new socket is bound; on bind failure the maps are
    /// left unchanged and an error is returned.
    ///
    /// When a bounded `udp_port_pool` is configured and `addr.port() == 0`,
    /// the driver scans the pool for an available port instead of relying on
    /// the OS ephemeral allocator.
    ///
    /// The returned `PortLease` must be `commit()`-ed once the caller is sure the
    /// socket should be kept (e.g. after `RtpCore` accepted the session). If the
    /// lease is dropped without being committed, the reference count is decremented
    /// and the socket is removed/cancelled when it reaches zero.
    pub(crate) async fn acquire(&self, addr: SocketAddr, reuse: bool) -> Result<PortLease, String> {
        let sockets = self.inner.sockets.lock().await;
        let should_reuse = reuse && addr.port() != 0;

        if should_reuse {
            if let Some(socket) = sockets.get(&addr) {
                let actual = socket.local_addr().unwrap_or(addr);
                drop(sockets);
                let mut counts = self.inner.counts.lock().await;
                *counts.entry(actual).or_insert(0) += 1;
                return Ok(PortLease {
                    manager: self.clone(),
                    addr: actual,
                    committed: false,
                });
            }
        }

        drop(sockets);

        let bind_addrs = self.bind_candidates(addr);
        let mut last_error = String::new();
        for candidate in bind_addrs {
            match self.try_bind(candidate).await {
                Ok(lease) => return Ok(lease),
                Err(reason) => last_error = reason,
            }
        }

        let reason = if last_error.is_empty() {
            format!("failed to bind UDP socket {addr}: no port available")
        } else {
            format!("failed to bind UDP socket {addr}: {last_error}")
        };
        error!("{reason}");
        Err(reason)
    }

    /// Build the list of addresses to attempt binding, applying the configured
    /// port pool when the requested port is `0`.
    fn bind_candidates(&self, addr: SocketAddr) -> Vec<SocketAddr> {
        if addr.port() != 0 {
            return vec![addr];
        }
        if let Some(pool) = self.inner.udp_port_pool {
            let start = pool.start;
            let end = pool.end;
            let offset =
                self.inner.next_port.fetch_add(1, Ordering::Relaxed) as u32 % (pool.count() as u32);
            let first = (start as u32 + offset) as u16;
            let mut ports: Vec<u16> = (first..=end).collect();
            if start < first {
                ports.extend(start..first);
            }
            return ports
                .into_iter()
                .map(|p| SocketAddr::new(addr.ip(), p))
                .collect();
        }
        vec![addr]
    }

    /// Try to bind a single UDP socket and register it.
    async fn try_bind(&self, addr: SocketAddr) -> Result<PortLease, String> {
        match UdpSocket::bind(addr).await {
            Ok(s) => {
                let actual = s.local_addr().unwrap_or(addr);
                let socket = Arc::new(s);
                let socket_cancel = self.inner.cancel.child_token();
                spawn_udp_reader(
                    socket.clone(),
                    socket_cancel.clone(),
                    self.inner.udp_tx.clone(),
                    self.inner.rtcp_tx.clone(),
                    self.inner.rtcp_mux,
                    self.inner.read_buffer_size,
                    self.inner.runtime.clone(),
                    self.inner.load_limiter.clone(),
                );

                self.inner.sockets.lock().await.insert(actual, socket);
                self.inner
                    .cancels
                    .lock()
                    .await
                    .insert(actual, socket_cancel);
                self.inner.counts.lock().await.insert(actual, 1);

                debug!("acquired UDP socket lease on {actual}");
                Ok(PortLease {
                    manager: self.clone(),
                    addr: actual,
                    committed: false,
                })
            }
            Err(e) => Err(format!("{e}")),
        }
    }

    /// Release one reference to the socket bound at `addr`.
    pub(crate) async fn release(&self, addr: SocketAddr) {
        let mut counts = self.inner.counts.lock().await;
        if let Some(count) = counts.get_mut(&addr) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(&addr);
                drop(counts);
                self.inner.sockets.lock().await.remove(&addr);
                if let Some(token) = self.inner.cancels.lock().await.remove(&addr) {
                    token.cancel();
                }
                debug!("released UDP socket lease on {addr}");
            }
        }
    }

    /// Return the socket bound at `addr` if one exists, otherwise `None`.
    pub(crate) async fn get_socket(&self, addr: SocketAddr) -> Option<Arc<UdpSocket>> {
        self.inner.sockets.lock().await.get(&addr).cloned()
    }
}

/// RAII guard for a per-session UDP socket lease.
///
/// Dropping the guard without calling `commit()` returns the lease so the socket
/// is released if no other session holds it. This makes bind failures and core
/// rejections safe: the socket is only kept once the caller explicitly commits.
pub(crate) struct PortLease {
    manager: PortManager,
    addr: SocketAddr,
    committed: bool,
}

impl PortLease {
    pub(crate) fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Keep the socket bound; the guard will no longer release it on drop.
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for PortLease {
    fn drop(&mut self) {
        if !self.committed {
            let manager = self.manager.clone();
            let addr = self.addr;
            tokio::spawn(async move {
                manager.release(addr).await;
            });
        }
    }
}
