//! Real MP4 file end-to-end tests for the `PlaybackApi` provider.
//!
//! These tests write an actual MP4 file to disk, start a `VodApi` with a
//! `TokioRuntime`, and observe the bridged frames through a fake
//! `CoreAdaptersApi`. They assert track/frame order, pause/resume behaviour,
//! seek discontinuity, scale acceptance and EOF/stop/restart cleanup.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecExtradata, CodecId, FrameFlags, MediaKind, Mp4WriteEvent, Mp4Writer,
    Mp4WriterConfig, TrackId, TrackInfo,
};
use cheetah_mp4_module::playback_provider::Mp4PlaybackProvider;
use cheetah_mp4_module::{Mp4ModuleConfig, VodApi, VodSessionRegistry};
use cheetah_runtime_api::RuntimeApi;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::media_api::command::{OpenPlaybackRequest, PlaybackControl};
use cheetah_sdk::media_api::error::Result as MediaResult;
use cheetah_sdk::media_api::ids::{FileHandle, MediaKey, PlaybackSessionId};
use cheetah_sdk::media_api::media_file_store::{
    DeleteBatchResult, FileDownload, FileRange, FileStoreEntry, FileStoreQuery, MediaFileStoreApi,
};
use cheetah_sdk::media_api::model::{PlaybackSession, PlaybackSessionState};
use cheetah_sdk::media_api::port::{MediaRequestContext, PlaybackApi};
use cheetah_sdk::{CoreAdaptersApi, DispatchResult, SdkError, StreamKey};
use tokio::io::AsyncWriteExt;
use tokio::time::timeout;

fn h264_track() -> TrackInfo {
    let mut t = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
    t.width = Some(640);
    t.height = Some(360);
    t.extradata = CodecExtradata::H264 {
        sps: vec![],
        pps: vec![],
        avcc: Some(Bytes::from_static(&[
            0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
            0x03, 0x68, 0xCE, 0x38,
        ])),
    };
    t
}

async fn write_test_mp4(path: &PathBuf, frame_count: usize) {
    let mut writer = Mp4Writer::new(Mp4WriterConfig::default(), &[h264_track()]).unwrap();
    for i in 0..frame_count {
        writer
            .push_sample(
                1,
                (i * 33_333) as i64,
                (i * 33_333) as i64,
                i == 0 || i == frame_count - 1,
                b"AU",
            )
            .unwrap();
    }
    let Mp4WriteEvent::File(buf) = writer.finalize().unwrap();
    let mut f = tokio::fs::File::create(path).await.unwrap();
    f.write_all(&buf).await.unwrap();
    f.sync_all().await.unwrap();
}

fn unique_temp_dir() -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut p = std::env::temp_dir();
    p.push(format!("cheetah-vod-e2e-{}-{}", std::process::id(), now));
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(&p).unwrap()
}

#[derive(Default)]
struct FakeCoreState {
    tracks: Vec<Vec<TrackInfo>>,
    frames: Vec<AVFrame>,
    closed_count: usize,
}

struct FakeCoreAdapters {
    state: Mutex<FakeCoreState>,
}

impl FakeCoreAdapters {
    fn new() -> Self {
        Self {
            state: Mutex::new(FakeCoreState::default()),
        }
    }

    fn frame_count(&self) -> usize {
        self.state.lock().unwrap().frames.len()
    }

    fn closed_count(&self) -> usize {
        self.state.lock().unwrap().closed_count
    }

    fn frames(&self) -> Vec<AVFrame> {
        self.state.lock().unwrap().frames.clone()
    }

    async fn wait_for_tracks(&self) -> Vec<TrackInfo> {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(t) = self.state.lock().unwrap().tracks.first().cloned() {
                    return t;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("timed out waiting for tracks")
    }

    async fn wait_for_frames(&self, min: usize) -> Vec<AVFrame> {
        timeout(Duration::from_secs(5), async {
            loop {
                let frames = self.frames();
                if frames.len() >= min {
                    return frames;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("timed out waiting for frames")
    }

    async fn wait_for_at_least_one_close(&self) {
        timeout(Duration::from_secs(5), async {
            while self.closed_count() == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("timed out waiting for close");
    }
}

#[async_trait]
impl CoreAdaptersApi for FakeCoreAdapters {
    async fn publish_frame(
        &self,
        _stream_key: StreamKey,
        frame: Arc<AVFrame>,
    ) -> Result<DispatchResult, SdkError> {
        self.state.lock().unwrap().frames.push((*frame).clone());
        Ok(DispatchResult::Accepted)
    }

    async fn update_tracks(
        &self,
        _stream_key: StreamKey,
        tracks: Vec<TrackInfo>,
    ) -> Result<(), SdkError> {
        self.state.lock().unwrap().tracks.push(tracks);
        Ok(())
    }

    async fn close_stream(&self, _stream_key: &StreamKey) -> Result<(), SdkError> {
        self.state.lock().unwrap().closed_count += 1;
        Ok(())
    }
}

struct FakeFileStore {
    handle: String,
    path: PathBuf,
    media_key: MediaKey,
}

impl FakeFileStore {
    fn new(handle: &str, path: PathBuf) -> Self {
        Self {
            handle: handle.to_string(),
            path,
            media_key: MediaKey::with_default_vhost("*", "*", None).unwrap(),
        }
    }
}

impl MediaFileStoreApi for FakeFileStore {
    fn register_file(
        &self,
        _ctx: &MediaRequestContext,
        _entry: FileStoreEntry,
    ) -> MediaResult<FileHandle> {
        unimplemented!()
    }

    fn resolve_for_read(
        &self,
        _ctx: &MediaRequestContext,
        handle: &FileHandle,
        _resource_scope: Option<&MediaKey>,
        _now_ms: i64,
    ) -> MediaResult<FileStoreEntry> {
        if handle.0 != self.handle {
            return Err(cheetah_sdk::media_api::error::MediaError::not_found(
                "unknown file handle",
            ));
        }
        let size = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        Ok(FileStoreEntry {
            media_key: self.media_key.clone(),
            file_type: "mp4".to_string(),
            content_type: "video/mp4".to_string(),
            size_bytes: size,
            created_at_ms: 0,
            expires_at_ms: None,
            absolute_path: self.path.to_string_lossy().to_string(),
            owner_principal: None,
            allowed_principals: vec![],
        })
    }

    fn delete(
        &self,
        _ctx: &MediaRequestContext,
        _handle: &FileHandle,
        _now_ms: i64,
    ) -> MediaResult<()> {
        unimplemented!()
    }

    fn delete_batch(
        &self,
        _ctx: &MediaRequestContext,
        _query: FileStoreQuery,
        _batch_limit: u32,
        _now_ms: i64,
    ) -> MediaResult<DeleteBatchResult> {
        unimplemented!()
    }

    fn resolve_download(
        &self,
        _ctx: &MediaRequestContext,
        _handle: &FileHandle,
        _range: Option<FileRange>,
        _filename: Option<String>,
        _now_ms: i64,
    ) -> MediaResult<FileDownload> {
        unimplemented!()
    }
}

struct Fixture {
    handle: FileHandle,
    playback: Arc<dyn PlaybackApi>,
    core: Arc<FakeCoreAdapters>,
}

impl Fixture {
    async fn new() -> Self {
        let root = unique_temp_dir();
        let file = root.join("test.mp4");
        write_test_mp4(&file, 3).await;

        let handle = FileHandle("vod-e2e-test".to_string());
        let file_store = Arc::new(FakeFileStore::new(&handle.0, file.clone()));

        let config = Mp4ModuleConfig {
            enabled: true,
            root_path: root.to_string_lossy().to_string(),
            max_sessions: 8,
            read_chunk_bytes: 256 * 1024,
            max_box_bytes: 8 * 1024 * 1024,
            idle_timeout_ms: 15_000,
        };

        let core = Arc::new(FakeCoreAdapters::new());
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let vod = Arc::new(VodApi::with_engine_bridge(
            Arc::new(VodSessionRegistry::new(8)),
            Arc::new(config),
            core.clone(),
            runtime,
        ));
        let playback = Arc::new(Mp4PlaybackProvider::new(
            vod,
            file_store,
            root.to_string_lossy(),
        ));

        Self {
            handle,
            playback,
            core,
        }
    }

    async fn open(&self) -> PlaybackSession {
        let media_key = MediaKey::with_default_vhost("playback", "vod", None).unwrap();
        self.playback
            .open_playback(
                &MediaRequestContext::default(),
                OpenPlaybackRequest {
                    file_handle: self.handle.clone(),
                    media_key,
                    start_position_ms: 0,
                    scale: 1.0,
                },
            )
            .await
            .unwrap()
    }

    async fn control(&self, id: &PlaybackSessionId, cmd: PlaybackControl) -> PlaybackSession {
        self.playback
            .control_playback(&MediaRequestContext::default(), id, cmd)
            .await
            .unwrap()
    }

    async fn stop(&self, id: &PlaybackSessionId) {
        self.playback
            .stop_playback(&MediaRequestContext::default(), id)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn playback_streams_tracks_and_frames_until_eof() {
    let fixture = Fixture::new().await;
    let session = fixture.open().await;
    assert_eq!(session.state, PlaybackSessionState::Playing);
    assert_eq!(session.scale, 1.0);

    let tracks = fixture.core.wait_for_tracks().await;
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].codec, CodecId::H264);
    assert_eq!(tracks[0].width, Some(640));

    let frames = fixture.core.wait_for_frames(3).await;
    assert_eq!(frames.len(), 3);
    assert!(
        frames.windows(2).all(|w| w[1].dts_us > w[0].dts_us),
        "dts must increase"
    );
    assert!(frames.iter().all(|f| f.media_kind == MediaKind::Video));

    fixture.core.wait_for_at_least_one_close().await;
    let final_session = fixture
        .playback
        .get_playback(&MediaRequestContext::default(), &session.session_id)
        .await;
    assert!(
        final_session.is_err() || final_session.unwrap().state == PlaybackSessionState::Completed
    );
}

#[tokio::test]
async fn pause_stops_new_frames_and_resume_continues() {
    let fixture = Fixture::new().await;
    let session = fixture.open().await;

    // Pause immediately; allow at most the first frame to race in before the
    // pause command is processed.
    let before = fixture.core.frame_count();
    let paused = fixture
        .control(&session.session_id, PlaybackControl::Pause)
        .await;
    assert_eq!(paused.state, PlaybackSessionState::Paused);

    tokio::time::sleep(Duration::from_millis(150)).await;
    let after_pause = fixture.core.frame_count();
    assert!(
        after_pause <= before + 1,
        "pause should stop new frames; got {before} before and {after_pause} after"
    );

    let resumed = fixture
        .control(&session.session_id, PlaybackControl::Resume)
        .await;
    assert_eq!(resumed.state, PlaybackSessionState::Playing);

    fixture.core.wait_for_frames(after_pause + 1).await;
    assert!(
        fixture.core.frame_count() > after_pause,
        "resume should deliver more frames"
    );
}

#[tokio::test]
async fn seek_restarts_output_from_target_with_discontinuity() {
    let fixture = Fixture::new().await;
    let session = fixture.open().await;

    // Seek to the final keyframe (~67 ms with three 33.333 ms samples).
    let target_ms = 67;
    let after_seek = fixture
        .control(
            &session.session_id,
            PlaybackControl::Seek {
                position_ms: target_ms,
            },
        )
        .await;
    assert_eq!(after_seek.position_ms, target_ms);

    let frames = fixture.core.wait_for_frames(1).await;
    let post_seek = frames
        .iter()
        .find(|f| f.pts_us >= target_ms * 1000 - 20_000)
        .expect("seek should produce a frame near the target");
    assert!(post_seek.flags.contains(FrameFlags::DISCONTINUITY));
}

#[tokio::test]
async fn scale_is_reflected_and_does_not_stop_playback() {
    let fixture = Fixture::new().await;
    let session = fixture.open().await;

    let scaled = fixture
        .control(
            &session.session_id,
            PlaybackControl::SetScale { scale: 2.0 },
        )
        .await;
    assert_eq!(scaled.scale, 2.0);

    fixture.core.wait_for_frames(1).await;
    assert!(fixture.core.frame_count() >= 1);

    let get = fixture
        .playback
        .get_playback(&MediaRequestContext::default(), &session.session_id)
        .await
        .unwrap();
    assert_eq!(get.scale, 2.0);
}

#[tokio::test]
async fn stop_closes_stream_and_restart_works() {
    let fixture = Fixture::new().await;
    let session = fixture.open().await;
    fixture.core.wait_for_tracks().await;
    fixture.core.wait_for_frames(2).await;

    fixture.stop(&session.session_id).await;
    fixture.core.wait_for_at_least_one_close().await;

    // A new session for the same file works and produces a fresh stream of frames.
    let session2 = fixture.open().await;
    assert_ne!(session.session_id.0, session2.session_id.0);
    fixture.core.wait_for_tracks().await;
    fixture
        .core
        .wait_for_frames(fixture.core.frame_count() + 2)
        .await;
}
