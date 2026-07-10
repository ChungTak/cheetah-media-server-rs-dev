use super::*;
use cheetah_sdk::AsyncUdpSocket;
use cheetah_sdk::StreamKey;
use std::collections::hash_map::Entry;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::config::RtspMulticastConfig;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MulticastTrackKey {
    stream_key: StreamKey,
    track_id: TrackId,
}

#[derive(Clone)]
pub(super) struct MulticastSender {
    pub destination: Ipv4Addr,
    pub rtp_port: u16,
    pub rtcp_port: u16,
    pub ttl: u8,
    pub rtp_socket: Arc<dyn AsyncUdpSocket>,
    pub rtcp_socket: Arc<dyn AsyncUdpSocket>,
}

impl MulticastSender {
    pub fn target_rtp(&self) -> SocketAddr {
        SocketAddr::new(self.destination.into(), self.rtp_port)
    }

    pub fn target_rtcp(&self) -> SocketAddr {
        SocketAddr::new(self.destination.into(), self.rtcp_port)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MulticastAcquireError {
    CapacityExhausted,
    SocketBindFailure,
    SocketOptionFailure,
}

impl MulticastAcquireError {
    pub fn rtsp_response(self) -> RtspErrorResponse {
        match self {
            Self::CapacityExhausted => (
                461,
                "Unsupported Transport",
                b"multicast sender pool exhausted",
            ),
            Self::SocketBindFailure => (
                500,
                "Internal Server Error",
                b"bind multicast sender socket failed",
            ),
            Self::SocketOptionFailure => (
                500,
                "Internal Server Error",
                b"configure multicast sender socket failed",
            ),
        }
    }
}

struct MulticastRegistryEntry {
    sender: Arc<MulticastSender>,
    sender_connection_id: RtspConnectionId,
    subscribers: HashSet<RtspConnectionId>,
    idle_release_deadline_micros: Option<u64>,
}

#[derive(Default)]
struct MulticastRegistryState {
    entries: HashMap<MulticastTrackKey, MulticastRegistryEntry>,
    pending_allocations: HashSet<(Ipv4Addr, u16)>,
    allocation_cursor: u64,
}

pub(super) struct MulticastSenderRegistry {
    config: RtspMulticastConfig,
    state: Arc<Mutex<MulticastRegistryState>>,
}

impl MulticastSenderRegistry {
    pub fn new(config: RtspMulticastConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(MulticastRegistryState::default())),
        }
    }

    pub fn acquire(
        &self,
        runtime_api: &Arc<dyn RuntimeApi>,
        now_micros: u64,
        connection_id: RtspConnectionId,
        stream_key: &StreamKey,
        track_id: TrackId,
    ) -> Result<Arc<MulticastSender>, MulticastAcquireError> {
        let key = MulticastTrackKey {
            stream_key: stream_key.clone(),
            track_id,
        };

        {
            let mut state = self.state.lock();
            Self::prune_expired_locked(&mut state, now_micros);
            if let Some(entry) = state.entries.get_mut(&key) {
                entry.subscribers.insert(connection_id);
                entry.idle_release_deadline_micros = None;
                if !entry.subscribers.contains(&entry.sender_connection_id) {
                    entry.sender_connection_id = connection_id;
                }
                return Ok(entry.sender.clone());
            }
            if state
                .entries
                .len()
                .saturating_add(state.pending_allocations.len())
                >= self.config.max_groups
            {
                return Err(MulticastAcquireError::CapacityExhausted);
            }
        }

        let (destination, rtp_port, rtcp_port) = {
            let mut state = self.state.lock();
            Self::prune_expired_locked(&mut state, now_micros);
            match self.allocate_group_port_pair_locked(&mut state) {
                Some(value) => {
                    state.pending_allocations.insert((value.0, value.1));
                    value
                }
                None => return Err(MulticastAcquireError::CapacityExhausted),
            }
        };

        let socket_result = (|| {
            let bind_addr = SocketAddr::new(IpAddr::V4(self.config.interface), 0);
            let rtp_socket: Arc<dyn AsyncUdpSocket> = runtime_api
                .bind_udp(bind_addr)
                .map(Arc::from)
                .map_err(|_| MulticastAcquireError::SocketBindFailure)?;
            let rtcp_socket: Arc<dyn AsyncUdpSocket> = runtime_api
                .bind_udp(bind_addr)
                .map(Arc::from)
                .map_err(|_| MulticastAcquireError::SocketBindFailure)?;

            let ttl = u32::from(self.config.ttl);
            rtp_socket
                .set_multicast_ttl_v4(ttl)
                .map_err(|_| MulticastAcquireError::SocketOptionFailure)?;
            rtcp_socket
                .set_multicast_ttl_v4(ttl)
                .map_err(|_| MulticastAcquireError::SocketOptionFailure)?;
            Ok((rtp_socket, rtcp_socket))
        })();

        let (rtp_socket, rtcp_socket) = match socket_result {
            Ok(sockets) => sockets,
            Err(err) => {
                let mut state = self.state.lock();
                state.pending_allocations.remove(&(destination, rtp_port));
                return Err(err);
            }
        };

        let sender = Arc::new(MulticastSender {
            destination,
            rtp_port,
            rtcp_port,
            ttl: self.config.ttl,
            rtp_socket,
            rtcp_socket,
        });

        let mut state = self.state.lock();
        state.pending_allocations.remove(&(destination, rtp_port));
        Self::prune_expired_locked(&mut state, now_micros);
        match state.entries.entry(key) {
            Entry::Occupied(mut occupied) => {
                occupied.get_mut().subscribers.insert(connection_id);
                occupied.get_mut().idle_release_deadline_micros = None;
                if !occupied
                    .get()
                    .subscribers
                    .contains(&occupied.get().sender_connection_id)
                {
                    occupied.get_mut().sender_connection_id = connection_id;
                }
                Ok(occupied.get().sender.clone())
            }
            Entry::Vacant(vacant) => {
                vacant.insert(MulticastRegistryEntry {
                    sender: sender.clone(),
                    sender_connection_id: connection_id,
                    subscribers: HashSet::from([connection_id]),
                    idle_release_deadline_micros: None,
                });
                Ok(sender)
            }
        }
    }

    pub fn release(
        &self,
        runtime_api: &Arc<dyn RuntimeApi>,
        now_micros: u64,
        connection_id: RtspConnectionId,
        stream_key: &StreamKey,
        track_id: TrackId,
    ) {
        let key = MulticastTrackKey {
            stream_key: stream_key.clone(),
            track_id,
        };

        let mut idle_deadline = None;
        {
            let mut state = self.state.lock();
            Self::prune_expired_locked(&mut state, now_micros);
            if let Some(entry) = state.entries.get_mut(&key) {
                let removed = entry.subscribers.remove(&connection_id);
                if removed {
                    if entry.subscribers.is_empty() {
                        let deadline = now_micros
                            .saturating_add(self.config.idle_release_ms.saturating_mul(1_000));
                        entry.idle_release_deadline_micros = Some(deadline);
                        idle_deadline = Some(deadline);
                    } else if entry.sender_connection_id == connection_id {
                        entry.sender_connection_id = entry
                            .subscribers
                            .iter()
                            .min()
                            .copied()
                            .unwrap_or(connection_id);
                    }
                }
            }
            Self::prune_expired_locked(&mut state, now_micros);
        }
        if let Some(deadline_micros) = idle_deadline {
            self.spawn_idle_prune_task(runtime_api, deadline_micros);
        }
    }

    pub fn should_forward_rtp(
        &self,
        connection_id: RtspConnectionId,
        stream_key: &StreamKey,
        track_id: TrackId,
    ) -> bool {
        let key = MulticastTrackKey {
            stream_key: stream_key.clone(),
            track_id,
        };
        let state = self.state.lock();
        state.entries.get(&key).is_some_and(|entry| {
            entry.sender_connection_id == connection_id
                && entry.subscribers.contains(&connection_id)
        })
    }

    fn spawn_idle_prune_task(&self, runtime_api: &Arc<dyn RuntimeApi>, deadline_micros: u64) {
        let runtime_api_for_task = runtime_api.clone();
        let state = self.state.clone();
        let _ = runtime_api.spawn(Box::pin(async move {
            let mut timer = runtime_api_for_task
                .sleep_until(cheetah_codec::MonoTime::from_micros(deadline_micros));
            timer.wait().await;
            let now_micros = runtime_api_for_task.now().as_micros();
            let mut guard = state.lock();
            Self::prune_expired_locked(&mut guard, now_micros);
        }));
    }

    fn prune_expired_locked(state: &mut MulticastRegistryState, now_micros: u64) {
        state.entries.retain(|_, entry| {
            if !entry.subscribers.is_empty() {
                return true;
            }
            match entry.idle_release_deadline_micros {
                Some(deadline) => deadline > now_micros,
                None => false,
            }
        });
    }

    fn allocate_group_port_pair_locked(
        &self,
        state: &mut MulticastRegistryState,
    ) -> Option<(Ipv4Addr, u16, u16)> {
        let group_start = u32::from(self.config.group_start);
        let group_end = u32::from(self.config.group_end);
        if group_end < group_start {
            return None;
        }
        let group_count = u64::from(group_end.saturating_sub(group_start)).saturating_add(1);

        let first_even_port = if self.config.port_start.is_multiple_of(2) {
            self.config.port_start
        } else {
            self.config.port_start.saturating_add(1)
        };
        if first_even_port.saturating_add(1) > self.config.port_end {
            return None;
        }
        let pair_count =
            u64::from(self.config.port_end.saturating_sub(first_even_port)).saturating_add(1) / 2;
        if pair_count == 0 {
            return None;
        }

        let total_combinations = group_count.saturating_mul(pair_count);
        if total_combinations == 0 {
            return None;
        }

        let mut in_use = HashSet::<(Ipv4Addr, u16)>::new();
        for entry in state.entries.values() {
            in_use.insert((entry.sender.destination, entry.sender.rtp_port));
        }
        in_use.extend(state.pending_allocations.iter().copied());

        let max_attempts = total_combinations.min(65_536);
        let start = state.allocation_cursor % total_combinations;
        for attempt in 0..max_attempts {
            let idx = (start + attempt) % total_combinations;
            let group_offset = idx % group_count;
            let port_offset = idx / group_count;
            let destination = Ipv4Addr::from(group_start.saturating_add(group_offset as u32));
            let rtp_port = first_even_port
                .saturating_add((port_offset.saturating_mul(2)).min(u64::from(u16::MAX)) as u16);
            let rtcp_port = rtp_port.saturating_add(1);
            if rtcp_port > self.config.port_end {
                continue;
            }
            if in_use.contains(&(destination, rtp_port)) {
                continue;
            }
            state.allocation_cursor = idx.saturating_add(1);
            return Some((destination, rtp_port, rtcp_port));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_codec::MonoTime;
    use cheetah_sdk::{
        AsyncTcpListener, AsyncTcpStream, AsyncTimer, JoinHandle, SpawnError, TaskJoinError,
        UdpRecvMeta,
    };
    use std::future::Future;
    use std::io;
    use std::net::{
        SocketAddr, TcpListener as StdTcpListener, TcpStream as StdTcpStream,
        UdpSocket as StdUdpSocket,
    };
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex as StdMutex};

    struct NullUdpSocket;

    #[async_trait::async_trait]
    impl AsyncUdpSocket for NullUdpSocket {
        async fn recv_from(&self, _buf: &mut [u8]) -> io::Result<UdpRecvMeta> {
            Err(io::Error::new(io::ErrorKind::WouldBlock, "null"))
        }

        async fn send_to(&self, buf: &[u8], _target: SocketAddr) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            Ok(SocketAddr::from(([0, 0, 0, 0], 0)))
        }

        fn set_multicast_ttl_v4(&self, _ttl: u32) -> io::Result<()> {
            Ok(())
        }
    }

    struct NoopJoinHandle;

    impl JoinHandle for NoopJoinHandle {
        fn abort(&self) {}

        fn is_finished(&self) -> bool {
            true
        }

        fn wait(
            self: Box<Self>,
        ) -> Pin<Box<dyn Future<Output = Result<(), TaskJoinError>> + Send + 'static>> {
            Box::pin(async { Ok(()) })
        }
    }

    struct NeverTimer;

    #[async_trait::async_trait]
    impl AsyncTimer for NeverTimer {
        async fn wait(&mut self) {}

        fn deadline(&self) -> MonoTime {
            MonoTime::from_micros(0)
        }
    }

    struct ImmediateTimer;

    #[async_trait::async_trait]
    impl AsyncTimer for ImmediateTimer {
        async fn wait(&mut self) {}

        fn deadline(&self) -> MonoTime {
            MonoTime::from_micros(0)
        }
    }

    struct BlockingRuntimeApi {
        bind_calls: AtomicUsize,
        first_bind_entered: (StdMutex<bool>, Condvar),
        release_first_bind: (StdMutex<bool>, Condvar),
    }

    impl BlockingRuntimeApi {
        fn new() -> Self {
            Self {
                bind_calls: AtomicUsize::new(0),
                first_bind_entered: (StdMutex::new(false), Condvar::new()),
                release_first_bind: (StdMutex::new(false), Condvar::new()),
            }
        }

        fn wait_for_first_bind(&self) {
            let (lock, cvar) = &self.first_bind_entered;
            let mut entered = lock.lock().expect("first bind entered lock");
            while !*entered {
                entered = cvar.wait(entered).expect("first bind entered wait");
            }
        }

        fn release_first_bind(&self) {
            let (lock, cvar) = &self.release_first_bind;
            *lock.lock().expect("release first bind lock") = true;
            cvar.notify_all();
        }
    }

    impl RuntimeApi for BlockingRuntimeApi {
        fn now(&self) -> MonoTime {
            MonoTime::from_micros(0)
        }

        fn spawn(
            &self,
            _fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        ) -> Box<dyn JoinHandle> {
            Box::new(NoopJoinHandle)
        }

        fn spawn_local(
            &self,
            _fut: Pin<Box<dyn Future<Output = ()> + 'static>>,
        ) -> Result<Box<dyn JoinHandle>, SpawnError> {
            Ok(Box::new(NoopJoinHandle))
        }

        fn bind_udp(&self, _addr: SocketAddr) -> io::Result<Box<dyn AsyncUdpSocket>> {
            if self.bind_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let (entered_lock, entered_cvar) = &self.first_bind_entered;
                *entered_lock.lock().expect("first bind entered lock") = true;
                entered_cvar.notify_all();

                let (release_lock, release_cvar) = &self.release_first_bind;
                let mut released = release_lock.lock().expect("release first bind lock");
                while !*released {
                    released = release_cvar
                        .wait(released)
                        .expect("release first bind wait");
                }
            }
            Ok(Box::new(NullUdpSocket))
        }

        fn connect_tcp(&self, _addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpStream>> {
            unimplemented!("not used by multicast registry tests")
        }

        fn bind_tcp(&self, _addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpListener>> {
            unimplemented!("not used by multicast registry tests")
        }

        fn wrap_udp_socket(&self, _socket: StdUdpSocket) -> io::Result<Box<dyn AsyncUdpSocket>> {
            unimplemented!("not used by multicast registry tests")
        }

        fn wrap_tcp_listener(
            &self,
            _listener: StdTcpListener,
        ) -> io::Result<Box<dyn AsyncTcpListener>> {
            unimplemented!("not used by multicast registry tests")
        }

        fn wrap_tcp_stream(&self, _stream: StdTcpStream) -> io::Result<Box<dyn AsyncTcpStream>> {
            unimplemented!("not used by multicast registry tests")
        }

        fn sleep_until(&self, _deadline: MonoTime) -> Box<dyn AsyncTimer> {
            Box::new(NeverTimer)
        }
    }

    struct InlineRuntimeApi {
        now_micros: u64,
        bind_addrs: Arc<StdMutex<Vec<SocketAddr>>>,
    }

    impl InlineRuntimeApi {
        fn new(now_micros: u64) -> Self {
            Self {
                now_micros,
                bind_addrs: Arc::new(StdMutex::new(Vec::new())),
            }
        }

        fn with_bind_addrs(now_micros: u64, bind_addrs: Arc<StdMutex<Vec<SocketAddr>>>) -> Self {
            Self {
                now_micros,
                bind_addrs,
            }
        }
    }

    impl RuntimeApi for InlineRuntimeApi {
        fn now(&self) -> MonoTime {
            MonoTime::from_micros(self.now_micros)
        }

        fn spawn(
            &self,
            fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        ) -> Box<dyn JoinHandle> {
            futures::executor::block_on(fut);
            Box::new(NoopJoinHandle)
        }

        fn spawn_local(
            &self,
            _fut: Pin<Box<dyn Future<Output = ()> + 'static>>,
        ) -> Result<Box<dyn JoinHandle>, SpawnError> {
            Ok(Box::new(NoopJoinHandle))
        }

        fn bind_udp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncUdpSocket>> {
            self.bind_addrs
                .lock()
                .expect("bind addr recorder lock")
                .push(addr);
            Ok(Box::new(NullUdpSocket))
        }

        fn connect_tcp(&self, _addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpStream>> {
            unimplemented!("not used by multicast registry tests")
        }

        fn bind_tcp(&self, _addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpListener>> {
            unimplemented!("not used by multicast registry tests")
        }

        fn wrap_udp_socket(&self, _socket: StdUdpSocket) -> io::Result<Box<dyn AsyncUdpSocket>> {
            unimplemented!("not used by multicast registry tests")
        }

        fn wrap_tcp_listener(
            &self,
            _listener: StdTcpListener,
        ) -> io::Result<Box<dyn AsyncTcpListener>> {
            unimplemented!("not used by multicast registry tests")
        }

        fn wrap_tcp_stream(&self, _stream: StdTcpStream) -> io::Result<Box<dyn AsyncTcpStream>> {
            unimplemented!("not used by multicast registry tests")
        }

        fn sleep_until(&self, _deadline: MonoTime) -> Box<dyn AsyncTimer> {
            Box::new(ImmediateTimer)
        }
    }

    fn single_slot_config() -> RtspMulticastConfig {
        RtspMulticastConfig {
            enabled: true,
            group_start: Ipv4Addr::new(239, 10, 0, 1),
            group_end: Ipv4Addr::new(239, 10, 0, 1),
            port_start: 63000,
            port_end: 63001,
            ttl: 8,
            interface: Ipv4Addr::UNSPECIFIED,
            idle_release_ms: 100,
            max_groups: 1,
        }
    }

    #[test]
    fn pending_multicast_allocation_counts_against_capacity() {
        let registry = Arc::new(MulticastSenderRegistry::new(single_slot_config()));
        let runtime = Arc::new(BlockingRuntimeApi::new());
        let stream_key = StreamKey::new("live", "race");

        let first_registry = registry.clone();
        let first_runtime: Arc<dyn RuntimeApi> = runtime.clone();
        let first_stream_key = stream_key.clone();
        let first = std::thread::spawn(move || {
            first_registry.acquire(&first_runtime, 1, 101, &first_stream_key, TrackId(1))
        });

        runtime.wait_for_first_bind();
        let second_runtime: Arc<dyn RuntimeApi> = runtime.clone();
        let second = registry.acquire(&second_runtime, 1, 102, &stream_key, TrackId(2));
        assert!(matches!(
            second,
            Err(MulticastAcquireError::CapacityExhausted)
        ));

        runtime.release_first_bind();
        assert!(
            first.join().expect("first acquire thread").is_ok(),
            "pending allocation should still complete after capacity rejected a concurrent acquire"
        );
    }

    #[test]
    fn shared_multicast_track_uses_single_sender_with_handover() {
        let registry = MulticastSenderRegistry::new(single_slot_config());
        let runtime: Arc<dyn RuntimeApi> = Arc::new(InlineRuntimeApi::new(1_000_000));
        let stream_key = StreamKey::new("live", "shared");
        registry
            .acquire(&runtime, 1, 10, &stream_key, TrackId(1))
            .expect("first multicast acquire");
        registry
            .acquire(&runtime, 2, 20, &stream_key, TrackId(1))
            .expect("second multicast acquire");

        assert!(registry.should_forward_rtp(10, &stream_key, TrackId(1)));
        assert!(!registry.should_forward_rtp(20, &stream_key, TrackId(1)));

        registry.release(&runtime, 3, 10, &stream_key, TrackId(1));

        assert!(registry.should_forward_rtp(20, &stream_key, TrackId(1)));
        assert!(!registry.should_forward_rtp(10, &stream_key, TrackId(1)));
    }

    #[test]
    fn idle_multicast_sender_is_pruned_without_followup_activity() {
        let registry = MulticastSenderRegistry::new(single_slot_config());
        let runtime: Arc<dyn RuntimeApi> = Arc::new(InlineRuntimeApi::new(10_000_000));
        let stream_key = StreamKey::new("live", "idle");
        registry
            .acquire(&runtime, 1, 11, &stream_key, TrackId(1))
            .expect("acquire initial sender");
        registry.release(&runtime, 2, 11, &stream_key, TrackId(1));

        // max_groups == 1: this would fail if idle sender entry were not pruned.
        let second = registry.acquire(&runtime, 3, 22, &stream_key, TrackId(2));
        assert!(
            second.is_ok(),
            "scheduled idle prune should release slot without further acquire/release traffic"
        );
    }

    #[test]
    fn multicast_sender_binds_to_configured_interface() {
        let mut config = single_slot_config();
        config.interface = Ipv4Addr::LOCALHOST;
        let registry = MulticastSenderRegistry::new(config);
        let bind_addrs = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeApi> = Arc::new(InlineRuntimeApi::with_bind_addrs(
            1_000_000,
            bind_addrs.clone(),
        ));
        let stream_key = StreamKey::new("live", "interface");

        registry
            .acquire(&runtime, 1, 33, &stream_key, TrackId(1))
            .expect("multicast acquire");

        let bind_addrs = bind_addrs.lock().expect("bind addr recorder lock");
        assert_eq!(
            bind_addrs.as_slice(),
            &[
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            ]
        );
    }
}
