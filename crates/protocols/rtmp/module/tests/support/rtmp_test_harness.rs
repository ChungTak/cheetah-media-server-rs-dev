use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cheetah_config::ConfigStore;
use cheetah_engine::{Engine, EngineBuilder};
use cheetah_rtmp_core::{RtmpClientState, RtmpEvent, RtmpMediaType, RtmpUrl};
use cheetah_rtmp_driver_tokio::{
    start_client, ClientDriverEvent, RtmpClientDriverConfig, RtmpClientHandle, RtmpClientMode,
};
use cheetah_rtmp_module::RtmpModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{CancellationToken, ModuleId, ModuleState, StreamKey, StreamSnapshot};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

use super::capture_fixture::CapturePublishCase;

const COALESCED_PAIR_START_RECORD: usize = 12;
const START_BIND_RETRIES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawReplayMode {
    RecordBoundaries,
    Coalesced,
}

pub struct RtmpTestHarness {
    engine: Engine,
    runtime: Arc<TokioRuntime>,
    listen: SocketAddr,
    raw_connections: Mutex<Vec<RawConnection>>,
}

struct RawConnection {
    writer: OwnedWriteHalf,
    reader: JoinHandle<()>,
}

pub struct RawPublishSession {
    case_name: &'static str,
    writer: OwnedWriteHalf,
    reader: JoinHandle<()>,
    remaining_records: Vec<&'static [u8]>,
}

impl RawPublishSession {
    pub async fn finish_remaining(&mut self) {
        for record in self.remaining_records.drain(..) {
            self.writer.write_all(record).await.unwrap_or_else(|err| {
                panic!("write remaining raw record for {}: {err}", self.case_name)
            });
        }
        self.writer.flush().await.unwrap_or_else(|err| {
            panic!("flush remaining raw replay for {}: {err}", self.case_name)
        });
    }

    pub async fn shutdown(mut self) {
        let _ = self.writer.shutdown().await;
        let _ = timeout(Duration::from_secs(1), &mut self.reader).await;
    }
}

#[derive(Debug, Default)]
pub struct PlayMediaObservation {
    pub audio_timestamps_ms: Vec<u32>,
    pub video_timestamps_ms: Vec<u32>,
    pub saw_video_config: bool,
    pub saw_video_coded: bool,
    pub video_coded_after_playing: bool,
}

impl RtmpTestHarness {
    pub async fn start() -> Self {
        let runtime = Arc::new(TokioRuntime::new());
        for attempt in 0..START_BIND_RETRIES {
            let listen = reserve_listen_addr();
            let config = Arc::new(ConfigStore::new());
            let config_yaml = format!("modules:\n  rtmp:\n    listen: \"{listen}\"\n");
            config
                .load_yaml_str(&config_yaml)
                .expect("load rtmp config");

            let engine = EngineBuilder::new(config.clone(), config.clone(), runtime.clone())
                .with_config_schema_registry(config)
                .register_module_factory(Arc::new(RtmpModuleFactory))
                .build()
                .expect("build engine");

            match engine.start().await {
                Ok(()) => {
                    let harness = Self {
                        engine,
                        runtime: runtime.clone(),
                        listen,
                        raw_connections: Mutex::new(Vec::new()),
                    };
                    harness.wait_for_module_state(ModuleState::Running).await;
                    return harness;
                }
                Err(err) if attempt + 1 < START_BIND_RETRIES && is_addr_in_use_error(&err) => {
                    // The reserved ephemeral port may be taken before the module binds it.
                    sleep(Duration::from_millis(20)).await;
                }
                Err(err) => {
                    panic!("start engine with rtmp listen {listen}: {err:?}");
                }
            }
        }
        panic!("unable to start RTMP test harness after {START_BIND_RETRIES} bind attempts")
    }

    pub async fn replay_raw_publish(&self, case: &CapturePublishCase, mode: RawReplayMode) {
        self.replay_raw_publish_with_limit(case, mode, None).await;
    }

    pub async fn replay_raw_fault_chunks(&self, case_name: &str, chunks: Vec<Vec<u8>>) {
        assert!(
            !chunks.is_empty(),
            "fault replay for {case_name} must contain at least one chunk"
        );
        let stream = connect_with_retry(self.listen).await;
        let (reader, mut writer) = stream.into_split();
        let reader = tokio::spawn(drain_server_responses(reader));
        for chunk in chunks {
            if writer.write_all(&chunk).await.is_err() {
                break;
            }
        }
        let _ = writer.flush().await;
        self.raw_connections
            .lock()
            .await
            .push(RawConnection { writer, reader });
    }

    pub async fn start_raw_publish_prefix(
        &self,
        case: &CapturePublishCase,
        record_limit: usize,
    ) -> RawPublishSession {
        let mut records = case.records();
        assert!(
            !records.is_empty(),
            "fixture {} must contain at least one raw TCP record",
            case.name
        );
        let split_at = record_limit.clamp(1, records.len());
        let remaining_records = records.split_off(split_at);

        let stream = connect_with_retry(self.listen).await;
        let (reader, mut writer) = stream.into_split();
        let reader = tokio::spawn(drain_server_responses(reader));
        for record in records {
            writer
                .write_all(record)
                .await
                .unwrap_or_else(|err| panic!("write raw prefix for {}: {err}", case.name));
        }
        writer
            .flush()
            .await
            .unwrap_or_else(|err| panic!("flush raw prefix for {}: {err}", case.name));

        RawPublishSession {
            case_name: case.name,
            writer,
            reader,
            remaining_records,
        }
    }

    async fn replay_raw_publish_with_limit(
        &self,
        case: &CapturePublishCase,
        mode: RawReplayMode,
        record_limit: Option<usize>,
    ) {
        let records = case.records();
        assert!(
            !records.is_empty(),
            "fixture {} must contain at least one raw TCP record",
            case.name
        );
        let records = match record_limit {
            Some(limit) => records
                .into_iter()
                .take(limit.max(1))
                .collect::<Vec<&'static [u8]>>(),
            None => records,
        };

        let stream = connect_with_retry(self.listen).await;
        let (reader, mut writer) = stream.into_split();
        let reader = tokio::spawn(drain_server_responses(reader));
        match mode {
            RawReplayMode::RecordBoundaries => {
                for record in records {
                    writer
                        .write_all(record)
                        .await
                        .unwrap_or_else(|err| panic!("write raw record for {}: {err}", case.name));
                }
            }
            RawReplayMode::Coalesced => {
                // Keep handshake and early command/status causality intact; model TCP sticky packets
                // on adjacent post-control payload records.
                let sticky_pair_start = records.len().min(COALESCED_PAIR_START_RECORD);
                let mut index = 0;
                while index < records.len() {
                    if index == sticky_pair_start && index + 1 < records.len() {
                        let pair = &records[index..index + 2];
                        let total_len = pair.iter().map(|record| record.len()).sum();
                        let mut coalesced = Vec::with_capacity(total_len);
                        for record in pair {
                            coalesced.extend_from_slice(record);
                        }
                        writer.write_all(&coalesced).await.unwrap_or_else(|err| {
                            panic!("write coalesced raw replay for {}: {err}", case.name)
                        });
                        index += 2;
                    } else {
                        writer
                            .write_all(records[index])
                            .await
                            .unwrap_or_else(|err| {
                                panic!("write stable raw record for {}: {err}", case.name)
                            });
                        index += 1;
                    }
                }
            }
        }
        writer
            .flush()
            .await
            .unwrap_or_else(|err| panic!("flush raw replay for {}: {err}", case.name));
        self.raw_connections
            .lock()
            .await
            .push(RawConnection { writer, reader });
    }

    pub fn start_play_client(&self, stream_key: &StreamKey) -> RtmpClientHandle {
        let url = RtmpUrl::parse(&format!(
            "rtmp://{}/{}/{}",
            self.listen, stream_key.namespace, stream_key.path
        ))
        .unwrap_or_else(|err| panic!("build play URL for stream {stream_key}: {err}"));
        start_client(
            self.runtime.clone(),
            url,
            RtmpClientMode::Play,
            RtmpClientDriverConfig::default(),
            CancellationToken::new(),
        )
        .expect("start rtmp play client")
    }

    pub async fn wait_for_playing(&self, client: &mut RtmpClientHandle, stage: &str) {
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut saw_connected = false;
        let mut seen_events = Vec::new();
        loop {
            let now = Instant::now();
            assert!(
                now < deadline,
                "timeout waiting for RTMP play client Playing at {stage}, saw_connected={saw_connected}, seen_events={seen_events:?}"
            );
            let remaining = deadline.saturating_duration_since(now);
            let event = match timeout(remaining, client.recv_event()).await {
                Ok(Some(event)) => event,
                Ok(None) => panic!(
                    "RTMP play client event stream closed before Playing at {stage}, saw_connected={saw_connected}"
                ),
                Err(_) => panic!(
                    "timeout waiting RTMP play event before Playing at {stage}, saw_connected={saw_connected}, seen_events={seen_events:?}"
                ),
            };
            match event {
                ClientDriverEvent::Connected { .. } => {
                    saw_connected = true;
                    seen_events.push("driver_connected".to_string());
                }
                ClientDriverEvent::Closed { reason } => {
                    panic!("RTMP play client closed before Playing at {stage}: {reason}");
                }
                ClientDriverEvent::Core {
                    event:
                        RtmpEvent::ClientStateChanged {
                            state: RtmpClientState::Playing,
                        },
                } => return,
                ClientDriverEvent::Core {
                    event: RtmpEvent::MediaData { .. },
                } => {
                    panic!("RTMP play client received media before Playing at {stage}");
                }
                ClientDriverEvent::Core { event } => {
                    seen_events.push(format!("{event:?}"));
                    if seen_events.len() > 16 {
                        seen_events.remove(0);
                    }
                }
            }
        }
    }

    pub async fn collect_play_media(
        &self,
        client: &mut RtmpClientHandle,
        case: &CapturePublishCase,
        timeout_after: Duration,
    ) -> PlayMediaObservation {
        let deadline = Instant::now() + timeout_after;
        let target_audio = if case.expect_audio { 3 } else { 0 };
        let target_video = if case.expect_video { 2 } else { 0 };
        let mut observation = PlayMediaObservation::default();

        loop {
            if observation.audio_timestamps_ms.len() >= target_audio
                && observation.video_timestamps_ms.len() >= target_video
            {
                return observation;
            }

            let now = Instant::now();
            assert!(
                now < deadline,
                "timeout collecting play media for {}, observation={:?}",
                case.name,
                observation
            );
            let remaining = deadline.saturating_duration_since(now);
            let event = match timeout(remaining, client.recv_event()).await {
                Ok(Some(event)) => event,
                Ok(None) => panic!(
                    "RTMP play client event stream closed while collecting media for {}, observation={:?}",
                    case.name, observation
                ),
                Err(_) => panic!(
                    "timeout waiting play media event for {}, observation={:?}",
                    case.name, observation
                ),
            };

            match event {
                ClientDriverEvent::Closed { reason } => {
                    panic!(
                        "RTMP play client closed while collecting media for {}: {reason}, observation={:?}",
                        case.name, observation
                    );
                }
                ClientDriverEvent::Core {
                    event:
                        RtmpEvent::MediaData {
                            media_type,
                            timestamp_ms,
                            payload,
                            ..
                        },
                } => match media_type {
                    RtmpMediaType::Audio => observation.audio_timestamps_ms.push(timestamp_ms),
                    RtmpMediaType::Video => {
                        let (is_config, is_coded) = classify_video_payload(&payload);
                        observation.saw_video_config |= is_config;
                        observation.saw_video_coded |= is_coded;
                        observation.video_coded_after_playing |= is_coded;
                        observation.video_timestamps_ms.push(timestamp_ms);
                    }
                    RtmpMediaType::Data => {}
                },
                ClientDriverEvent::Connected { .. } | ClientDriverEvent::Core { .. } => {}
            }
        }
    }

    pub async fn wait_for_active_published_stream(
        &self,
        timeout_after: Duration,
    ) -> StreamSnapshot {
        let deadline = Instant::now() + timeout_after;
        loop {
            let snapshots = self
                .engine
                .stream_manager_api()
                .list_streams()
                .await
                .expect("list streams");
            if let Some(snapshot) = snapshots
                .iter()
                .find(|snapshot| snapshot.publisher_active && !snapshot.tracks.is_empty())
            {
                return snapshot.clone();
            }

            let now = Instant::now();
            assert!(
                now < deadline,
                "timeout waiting for active published stream with tracks, snapshots={:?}",
                snapshots
            );
            sleep(Duration::from_millis(20)).await;
        }
    }

    pub async fn assert_rtmp_running_and_healthy(&self) {
        sleep(Duration::from_millis(80)).await;
        assert_eq!(
            self.rtmp_module_state(),
            Some(ModuleState::Running),
            "rtmp module must remain running after raw replay"
        );
        assert!(
            self.engine.health_api().is_live(),
            "engine health live must remain true after raw replay"
        );
        assert!(
            self.engine.health_api().is_ready(),
            "engine health ready must remain true after raw replay"
        );
    }

    pub async fn stop(self) {
        let mut raw_connections = self.raw_connections.lock().await;
        for raw in raw_connections.iter_mut() {
            let _ = raw.writer.shutdown().await;
            let _ = timeout(Duration::from_secs(1), &mut raw.reader).await;
        }
        raw_connections.clear();
        drop(raw_connections);

        self.engine.stop().await;
        self.wait_for_module_state(ModuleState::Stopped).await;
        assert!(!self.engine.health_api().is_live());
        assert!(!self.engine.health_api().is_ready());
    }

    async fn wait_for_module_state(&self, expected: ModuleState) {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if self.rtmp_module_state() == Some(expected) {
                return;
            }
            let now = Instant::now();
            assert!(
                now < deadline,
                "timeout waiting for rtmp module state {expected:?}, current={:?}",
                self.rtmp_module_state()
            );
            sleep(Duration::from_millis(20)).await;
        }
    }

    fn rtmp_module_state(&self) -> Option<ModuleState> {
        self.engine
            .module_manager_api()
            .modules()
            .into_iter()
            .find_map(|(module_id, state)| {
                if module_id == ModuleId::new("rtmp") {
                    Some(state)
                } else {
                    None
                }
            })
    }
}

fn reserve_listen_addr() -> SocketAddr {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);
    listen
}

fn is_addr_in_use_error(err: &impl core::fmt::Debug) -> bool {
    let message = format!("{err:?}").to_ascii_lowercase();
    message.contains("address already in use")
        || message.contains("addrinuse")
        || message.contains("os error 98")
        || message.contains("os error 48")
        || message.contains("os error 10048")
}

async fn connect_with_retry(listen: SocketAddr) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        match timeout(Duration::from_millis(250), TcpStream::connect(listen)).await {
            Ok(Ok(stream)) => return stream,
            Ok(Err(err)) if Instant::now() >= deadline => {
                panic!("connect raw RTMP TCP stream to {listen}: {err}");
            }
            Err(_) if Instant::now() >= deadline => {
                panic!("timeout connecting raw RTMP TCP stream to {listen}");
            }
            Ok(Err(_)) | Err(_) => sleep(Duration::from_millis(20)).await,
        }
    }
}

async fn drain_server_responses(mut reader: tokio::net::tcp::OwnedReadHalf) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

fn classify_video_payload(payload: &[u8]) -> (bool, bool) {
    if payload.is_empty() {
        return (false, false);
    }
    if payload[0] & 0x80 != 0 {
        let packet_type = payload[0] & 0x0f;
        return (packet_type == 0, packet_type == 1 || packet_type == 3);
    }
    if payload.len() < 2 {
        return (false, false);
    }
    (payload[1] == 0, payload[1] == 1)
}
