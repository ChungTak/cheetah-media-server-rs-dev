//! Sans-I/O MP4 VOD session state machine.
//!
//! `cheetah-mp4-core` 是 MP4 VOD（点播）会话的 Sans-I/O 状态机。
//! 它不负责文件 I/O、套接字或调用 `Instant::now()`，这些由驱动层处理。
//! 本 crate 对 VOD 会话生命周期建模：
//!
//! * `Start` — 开始会话、暴露轨道信息、请求初始读取。
//! * `Tick` — 以协商速率驱动按 paced 播放。
//! * `Seek` — 跳转到目标时间戳。
//! * `Pause` — 暂停 pacing，但不释放 reader 状态。
//! * `Scale` — 调整播放速率。
//! * `Stop` — 终止会话。
//!
//! 输入通过 `step(now_us, input)` 消费，输出以 `Vec<VodOutput>` 发出。

use cheetah_codec::{
    AVFrame, Mp4ReadEvent, Mp4ReadRequest, Mp4ReadResult, Mp4Reader, Mp4ReaderConfig, TrackInfo,
};

/// Session identifier (caller-assigned).
///
/// 会话标识符（由调用方分配）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VodSessionId(pub u64);

/// Splits an `app/stream` key into namespace + path components for the engine.
///
/// 将 `app/stream` 键拆分为命名空间与路径，供引擎使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamKeyParts {
    pub namespace: String,
    pub path: String,
}

/// `StreamKeyParts` parsing helper.
///
/// `StreamKeyParts` 解析辅助。
impl StreamKeyParts {
    /// Parse an input into `namespace`/`path` parts, defaulting namespace to "file".
    ///
    /// 将输入解析为 `namespace`/`path`，默认命名空间为 "file"。
    pub fn parse(input: &str) -> Self {
        if let Some((ns, p)) = input.split_once('/') {
            Self {
                namespace: ns.to_string(),
                path: p.to_string(),
            }
        } else {
            Self {
                namespace: "file".to_string(),
                path: input.to_string(),
            }
        }
    }
}

/// Session command from the protocol/control layer.
///
/// 协议/控制层发来的会话命令。
#[derive(Debug, Clone)]
pub enum VodControlCommand {
    Start { file_size: u64 },
    Seek { position_us: i64 },
    Pause(bool),
    Scale(f32),
    Stop,
}

/// Input event for the state machine.
///
/// 状态机的输入事件。
#[derive(Debug, Clone)]
pub enum VodCoreInput {
    Control(VodControlCommand),
    /// Driver fulfilled a previous `ReadAt` request.
    ReadAt(Mp4ReadResult),
    /// Time tick from the driver. `now_us` should be monotonic.
    Tick {
        now_us: u64,
    },
}

/// Output emitted by the state machine.
///
/// 状态机发出的输出。
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum VodOutput {
    /// Driver should issue this byte read.
    ReadAt(Mp4ReadRequest),
    /// Track info available; the protocol layer can advertise to peers.
    EmitTrackInfo(Vec<TrackInfo>),
    /// New media frame ready for consumption.
    EmitFrame(AVFrame),
    /// Schedule the next tick at `now_us + delay_us`.
    ScheduleTick { delay_us: u64 },
    /// Session ended; driver should release file handle.
    CloseSession,
    /// Non-fatal diagnostic surfaced to the protocol layer for audit and
    /// translated error responses (e.g. seek out-of-range).
    Diagnostic(VodDiagnostic),
}

/// Non-fatal session diagnostics. The driver forwards them to the module
/// layer where they become RTSP/RTMP error responses or HTTP `result.code`
/// fields, mirroring ABL's "明确错误" requirement for invalid seeks and
/// pause-state violations.
///
/// 非致命会话诊断。驱动将其转发给模块层，成为 RTSP/RTMP 错误响应或 HTTP `result.code` 字段，
/// 满足 ABL 对无效 seek 和暂停状态违规的“明确错误”要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VodDiagnostic {
    /// `Seek` requested a position outside `[0, duration_us]`.
    SeekOutOfRange { requested_us: i64, duration_us: i64 },
    /// A control command was rejected because the session is in a state
    /// that disallows it (e.g. seek while paused on certain ABL profiles).
    InvalidState { reason: &'static str },
}

/// Sans-I/O MP4 VOD session.
///
/// Sans-I/O MP4 VOD 会话。
pub struct VodSession {
    reader: Mp4Reader,
    state: SessionState,
    paused: bool,
    scale: f32,
    started_real_us: Option<u64>,
    started_media_us: i64,
    pending_seek_us: Option<i64>,
    tracks_emitted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Internal session lifecycle state.
///
/// 内部会话生命周期状态。
enum SessionState {
    Idle,
    Loading,
    Running,
    Stopped,
}

/// `VodSession` API: lifecycle, accessors, and stepping.
///
/// `VodSession` API：生命周期、访问与 step 驱动。
impl VodSession {
    /// Create a new idle session with the configured MP4 reader.
    ///
    /// 用配置好的 MP4 reader 创建新的空闲会话。
    pub fn new(config: Mp4ReaderConfig) -> Self {
        Self {
            reader: Mp4Reader::new(config),
            state: SessionState::Idle,
            paused: false,
            scale: 1.0,
            started_real_us: None,
            started_media_us: 0,
            pending_seek_us: None,
            tracks_emitted: false,
        }
    }

    /// Return the total duration in microseconds.
    ///
    /// 返回总时长（微秒）。
    pub fn duration_us(&self) -> i64 {
        self.reader.duration_us()
    }

    /// Return the parsed tracks for the session.
    ///
    /// 返回本会话的解析轨道。
    pub fn tracks(&self) -> &[TrackInfo] {
        self.reader.tracks()
    }

    /// Return true if the session is running.
    ///
    /// 返回会话是否正在运行。
    pub fn is_running(&self) -> bool {
        matches!(self.state, SessionState::Running)
    }

    /// Return true if the session is stopped.
    ///
    /// 返回会话是否已停止。
    pub fn is_stopped(&self) -> bool {
        matches!(self.state, SessionState::Stopped)
    }

    /// Process one input and produce the corresponding outputs.
    ///
    /// 处理一个输入并产生对应输出。
    pub fn step(&mut self, input: VodCoreInput) -> Vec<VodOutput> {
        match input {
            VodCoreInput::Control(cmd) => self.handle_control(cmd),
            VodCoreInput::ReadAt(result) => {
                self.reader.feed_bytes(result);
                self.drive(None)
            }
            VodCoreInput::Tick { now_us } => self.drive(Some(now_us)),
        }
    }

    /// Handle a control command and update session state.
    ///
    /// 处理控制命令并更新会话状态。
    fn handle_control(&mut self, cmd: VodControlCommand) -> Vec<VodOutput> {
        match cmd {
            VodControlCommand::Start { file_size } => {
                self.reader.set_file_size(file_size);
                self.state = SessionState::Loading;
                self.drive(None)
            }
            VodControlCommand::Seek { position_us } => {
                // ABL requires explicit out-of-range errors rather than
                // silently clamping. The reader can only seek inside
                // `[0, duration_us]`; anything outside is a hard error
                // surfaced via a diagnostic, and the session keeps its
                // current position.
                let duration_us = self.reader.duration_us();
                if position_us < 0 || (duration_us > 0 && position_us > duration_us) {
                    return vec![VodOutput::Diagnostic(VodDiagnostic::SeekOutOfRange {
                        requested_us: position_us,
                        duration_us,
                    })];
                }
                self.pending_seek_us = Some(position_us);
                self.reader.seek(position_us);
                self.started_real_us = None;
                self.started_media_us = position_us;
                vec![VodOutput::ScheduleTick { delay_us: 0 }]
            }
            VodControlCommand::Pause(p) => {
                self.paused = p;
                if p {
                    Vec::new()
                } else {
                    vec![VodOutput::ScheduleTick { delay_us: 0 }]
                }
            }
            VodControlCommand::Scale(s) => {
                self.scale = s.clamp(0.05, 32.0);
                Vec::new()
            }
            VodControlCommand::Stop => {
                self.state = SessionState::Stopped;
                vec![VodOutput::CloseSession]
            }
        }
    }

    /// Drive the MP4 reader and emit outputs (reads, tracks, frames, tick, EOF).
    ///
    /// 驱动 MP4 reader 并发出输出（读取、轨道、帧、tick、EOF）。
    fn drive(&mut self, now_us: Option<u64>) -> Vec<VodOutput> {
        if matches!(self.state, SessionState::Stopped) {
            return Vec::new();
        }
        if self.paused {
            return Vec::new();
        }
        let mut out = Vec::new();
        loop {
            let event = self.reader.step();
            match event {
                Mp4ReadEvent::NeedBytes(req) => {
                    out.push(VodOutput::ReadAt(req));
                    return out;
                }
                Mp4ReadEvent::Tracks(tracks) => {
                    if !self.tracks_emitted {
                        self.tracks_emitted = true;
                        out.push(VodOutput::EmitTrackInfo(tracks));
                    }
                    self.state = SessionState::Running;
                }
                Mp4ReadEvent::Frame(frame) => {
                    if let Some(now) = now_us {
                        let delay = self.frame_delay_us(&frame, now);
                        if delay > 0 {
                            // Frame too early — push back into reader by buffering
                            // outside? For simplicity, still emit and schedule next
                            // tick. The driver may also choose to delay handing
                            // the bytes back, but the core does not buffer frames.
                            out.push(VodOutput::EmitFrame(frame));
                            out.push(VodOutput::ScheduleTick { delay_us: delay });
                            return out;
                        }
                    }
                    out.push(VodOutput::EmitFrame(frame));
                }
                Mp4ReadEvent::Eof => {
                    self.state = SessionState::Stopped;
                    out.push(VodOutput::CloseSession);
                    return out;
                }
                Mp4ReadEvent::Idle => return out,
                Mp4ReadEvent::Diagnostic(_) => continue,
            }
        }
    }

    /// Compute the pacing delay before emitting this frame.
    ///
    /// 计算发出该帧前的 pacing 延迟。
    fn frame_delay_us(&mut self, frame: &AVFrame, now_us: u64) -> u64 {
        let started_real = *self.started_real_us.get_or_insert(now_us);
        if self.started_media_us == 0 && self.pending_seek_us.is_none() {
            self.started_media_us = frame.dts_us;
        }
        // Saturating sub guards against backwards-stepping frames after a
        // seek or multi-file boundary; clamp scale to a positive minimum so
        // a buggy upstream `Scale(0.0)` cannot trigger a divide-by-zero.
        let scale = (self.scale as f64).max(0.001);
        let media_offset_us = frame.dts_us.saturating_sub(self.started_media_us);
        let target_offset_us = (media_offset_us as f64 / scale) as i64;
        let target_real_us = (started_real as i64).saturating_add(target_offset_us);
        if target_real_us > now_us as i64 {
            (target_real_us - now_us as i64) as u64
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use cheetah_codec::{
        CodecExtradata, CodecId, MediaKind, Mp4WriteEvent, Mp4Writer, Mp4WriterConfig, TrackId,
    };

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

    fn build_test_mp4() -> Bytes {
        let mut w = Mp4Writer::new(Mp4WriterConfig::default(), &[h264_track()]).unwrap();
        for i in 0..3 {
            w.push_sample(1, i * 33_333, i * 33_333, i == 0, b"AU")
                .unwrap();
        }
        let Mp4WriteEvent::File(buf) = w.finalize().unwrap();
        buf
    }

    fn run_session_until_eof(buf: &[u8]) -> (Vec<TrackInfo>, usize) {
        let mut s = VodSession::new(Mp4ReaderConfig::default());
        let mut tracks = Vec::new();
        let mut frames = 0usize;
        let mut next_input = VodCoreInput::Control(VodControlCommand::Start {
            file_size: buf.len() as u64,
        });
        let mut iterations = 0;
        loop {
            iterations += 1;
            assert!(iterations < 200, "loop did not converge");
            let outputs = s.step(next_input.clone());
            let mut consumed = false;
            for ev in outputs {
                match ev {
                    VodOutput::ReadAt(req) => {
                        let end = (req.offset + req.length) as usize;
                        let data = Bytes::copy_from_slice(&buf[req.offset as usize..end]);
                        next_input = VodCoreInput::ReadAt(Mp4ReadResult {
                            offset: req.offset,
                            data,
                        });
                        consumed = true;
                    }
                    VodOutput::EmitTrackInfo(t) => {
                        tracks = t;
                    }
                    VodOutput::EmitFrame(_) => frames += 1,
                    VodOutput::ScheduleTick { .. } => {}
                    VodOutput::Diagnostic(_) => {}
                    VodOutput::CloseSession => return (tracks, frames),
                }
            }
            if !consumed {
                next_input = VodCoreInput::Tick { now_us: 0 };
            }
        }
    }

    #[test]
    fn vod_session_emits_tracks_and_frames() {
        let buf = build_test_mp4();
        let (tracks, frames) = run_session_until_eof(&buf);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].codec, CodecId::H264);
        assert_eq!(frames, 3);
    }

    #[test]
    fn pause_blocks_subsequent_steps() {
        let buf = build_test_mp4();
        let mut s = VodSession::new(Mp4ReaderConfig::default());
        let _ = s.step(VodCoreInput::Control(VodControlCommand::Start {
            file_size: buf.len() as u64,
        }));
        s.step(VodCoreInput::Control(VodControlCommand::Pause(true)));
        let outs = s.step(VodCoreInput::Tick { now_us: 1_000_000 });
        assert!(outs.is_empty());
    }

    #[test]
    fn stop_emits_close_session() {
        let mut s = VodSession::new(Mp4ReaderConfig::default());
        let outs = s.step(VodCoreInput::Control(VodControlCommand::Stop));
        assert!(matches!(outs.last(), Some(VodOutput::CloseSession)));
    }

    #[test]
    fn stream_key_parts_split_on_first_slash() {
        let p = StreamKeyParts::parse("live/test");
        assert_eq!(p.namespace, "live");
        assert_eq!(p.path, "test");
        let f = StreamKeyParts::parse("only");
        assert_eq!(f.namespace, "file");
        assert_eq!(f.path, "only");
    }

    #[test]
    fn seek_negative_position_emits_diagnostic() {
        let buf = build_test_mp4();
        let mut s = VodSession::new(Mp4ReaderConfig::default());
        // Drive reader through Start so duration_us is set.
        feed_until_streaming(&mut s, &buf);
        let outs = s.step(VodCoreInput::Control(VodControlCommand::Seek {
            position_us: -1,
        }));
        assert!(matches!(
            outs.first(),
            Some(VodOutput::Diagnostic(VodDiagnostic::SeekOutOfRange {
                requested_us: -1,
                ..
            }))
        ));
    }

    #[test]
    fn seek_past_duration_emits_diagnostic() {
        let buf = build_test_mp4();
        let mut s = VodSession::new(Mp4ReaderConfig::default());
        feed_until_streaming(&mut s, &buf);
        let dur = s.duration_us();
        assert!(dur > 0, "duration should be known after streaming starts");
        let outs = s.step(VodCoreInput::Control(VodControlCommand::Seek {
            position_us: dur + 1_000_000,
        }));
        assert!(matches!(
            outs.first(),
            Some(VodOutput::Diagnostic(VodDiagnostic::SeekOutOfRange { .. }))
        ));
    }

    #[test]
    fn seek_within_duration_succeeds() {
        let buf = build_test_mp4();
        let mut s = VodSession::new(Mp4ReaderConfig::default());
        feed_until_streaming(&mut s, &buf);
        let outs = s.step(VodCoreInput::Control(VodControlCommand::Seek {
            position_us: 0,
        }));
        // No SeekOutOfRange diagnostic; should yield a ScheduleTick.
        assert!(!outs.iter().any(|o| matches!(
            o,
            VodOutput::Diagnostic(VodDiagnostic::SeekOutOfRange { .. })
        )));
    }

    #[test]
    fn scale_clamp_allows_high_speed_playback() {
        let mut s = VodSession::new(Mp4ReaderConfig::default());
        // 32x is allowed; the driver layer is responsible for keyframe
        // gating above the configured threshold.
        let _ = s.step(VodCoreInput::Control(VodControlCommand::Scale(64.0)));
        // The session does not emit anything for the clamp itself, but
        // it should not panic on out-of-range scales.
    }

    /// Drive the session through the head/moov parsing phase so `duration_us`
    /// is populated. Useful for tests that need to interact with the
    /// session while it is in the `Streaming` state.
    fn feed_until_streaming(s: &mut VodSession, buf: &Bytes) {
        let mut next_input = VodCoreInput::Control(VodControlCommand::Start {
            file_size: buf.len() as u64,
        });
        for _ in 0..50 {
            let outputs = s.step(next_input.clone());
            let mut consumed = false;
            for o in outputs {
                if let VodOutput::ReadAt(req) = o {
                    let end = (req.offset + req.length) as usize;
                    next_input = VodCoreInput::ReadAt(Mp4ReadResult {
                        offset: req.offset,
                        data: Bytes::copy_from_slice(&buf[req.offset as usize..end]),
                    });
                    consumed = true;
                    break;
                }
            }
            if !consumed {
                if s.duration_us() > 0 {
                    return;
                }
                next_input = VodCoreInput::Tick { now_us: 0 };
            }
        }
    }
}
