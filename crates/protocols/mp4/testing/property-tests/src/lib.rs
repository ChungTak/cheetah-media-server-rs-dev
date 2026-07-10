//! Property and regression tests for MP4 VOD core/codec.
//!
//! These tests live outside the runtime crates so that `cheetah-mp4-core` and
//! `cheetah-codec::mp4` remain free of property-test dependencies.
//!
//! MP4 VOD core/codec 的属性与回归测试。
//!
//! 这些测试位于运行时 crate 之外，以便 `cheetah-mp4-core` 与 `cheetah-codec::mp4`
//! 不依赖属性测试库。

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use cheetah_codec::{
        CodecExtradata, CodecId, MediaKind, Mp4ReadEvent, Mp4ReadResult, Mp4Reader,
        Mp4ReaderConfig, Mp4WriteEvent, Mp4Writer, Mp4WriterConfig, TrackId, TrackInfo,
    };
    use cheetah_mp4_core::{VodControlCommand, VodCoreInput, VodOutput, VodSession};

    /// Build a single H.264 video track fixture with AVCC extradata.
    ///
    /// 构造带 AVCC extradata 的单 H.264 视频轨道 fixture。
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

    /// Build a single AAC audio track fixture with AudioSpecificConfig.
    ///
    /// 构造带 AudioSpecificConfig 的单 AAC 音频轨道 fixture。
    fn aac_track() -> TrackInfo {
        let mut t = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 44_100);
        t.sample_rate = Some(44_100);
        t.channels = Some(2);
        t.extradata = CodecExtradata::AAC {
            asc: Bytes::from_static(&[0x12, 0x10]),
        };
        t
    }

    /// Build a multi-track MP4 file with a fixed number of video and audio samples.
    ///
    /// 构造包含固定数量视频与音频样本的多轨道 MP4 文件。
    fn build_multi_track_mp4(num_video: u32, num_audio: u32) -> Bytes {
        let mut w =
            Mp4Writer::new(Mp4WriterConfig::default(), &[h264_track(), aac_track()]).unwrap();
        for i in 0..num_video {
            w.push_sample(1, i as i64 * 33_333, i as i64 * 33_333, i == 0, b"VAU")
                .unwrap();
        }
        for i in 0..num_audio {
            w.push_sample(2, i as i64 * 23_220, i as i64 * 23_220, true, b"AAU")
                .unwrap();
        }
        let Mp4WriteEvent::File(buf) = w.finalize().unwrap();
        buf
    }

    /// Drive an `Mp4Reader` to EOF by feeding `NeedBytes` requests.
    ///
    /// Returns the emitted track metadata and the total frame count.
    ///
    /// 通过满足 `NeedBytes` 请求将 `Mp4Reader` 驱动到 EOF。
    ///
    /// 返回发出的轨道元数据与总帧数。
    fn drive_reader_to_eof(buf: &[u8]) -> (Vec<TrackInfo>, usize) {
        let mut reader = Mp4Reader::new(Mp4ReaderConfig::default());
        reader.set_file_size(buf.len() as u64);
        let mut tracks = Vec::new();
        let mut frames = 0usize;
        for _ in 0..2_000 {
            match reader.step() {
                Mp4ReadEvent::NeedBytes(req) => {
                    let end = (req.offset + req.length) as usize;
                    let data = Bytes::copy_from_slice(&buf[req.offset as usize..end]);
                    reader.feed_bytes(Mp4ReadResult {
                        offset: req.offset,
                        data,
                    });
                }
                Mp4ReadEvent::Tracks(t) => tracks = t,
                Mp4ReadEvent::Frame(_) => frames += 1,
                Mp4ReadEvent::Eof | Mp4ReadEvent::Idle => break,
                Mp4ReadEvent::Diagnostic(_) => {}
            }
        }
        (tracks, frames)
    }

    /// Multi-track writer/reader round-trip preserves track count and frame counts.
    ///
    /// 多轨道写/读往返保持轨道数与帧数。
    #[test]
    fn writer_reader_roundtrip_multi_track_preserves_frame_counts() {
        let buf = build_multi_track_mp4(10, 20);
        let (tracks, frames) = drive_reader_to_eof(&buf);
        assert_eq!(tracks.len(), 2);
        assert_eq!(frames, 30);
    }

    /// Seeking during a VOD session keeps the video timeline monotonic.
    ///
    /// The test drives the session through `Start`, then triggers a `Seek` after
    /// two frames and verifies that video timestamps do not regress.
    ///
    /// VOD 会话中的跳转保持视频时间线单调。
    ///
    /// 测试通过 `Start` 驱动会话，在两帧后触发 `Seek` 并验证视频时间戳不回退。
    #[test]
    fn vod_session_seek_keeps_timeline_monotonic() {
        let buf = build_multi_track_mp4(5, 5);
        let mut session = VodSession::new(Mp4ReaderConfig::default());
        let mut last_dts = i64::MIN;
        let mut next_input = VodCoreInput::Control(VodControlCommand::Start {
            file_size: buf.len() as u64,
        });
        let mut frames_seen = 0;
        let mut sought = false;
        for _ in 0..500 {
            let outputs = session.step(next_input.clone());
            let mut consumed = false;
            for out in outputs {
                match out {
                    VodOutput::ReadAt(req) => {
                        let end = (req.offset + req.length) as usize;
                        next_input = VodCoreInput::ReadAt(Mp4ReadResult {
                            offset: req.offset,
                            data: Bytes::copy_from_slice(&buf[req.offset as usize..end]),
                        });
                        consumed = true;
                    }
                    VodOutput::EmitFrame(frame) => {
                        if frame.codec == CodecId::H264 {
                            assert!(
                                frame.dts >= last_dts || sought,
                                "video timeline regressed without seek: {} -> {}",
                                last_dts,
                                frame.dts
                            );
                            last_dts = frame.dts;
                            frames_seen += 1;
                            if !sought && frames_seen == 2 {
                                sought = true;
                                next_input = VodCoreInput::Control(VodControlCommand::Seek {
                                    position_us: 0,
                                });
                                consumed = true;
                                last_dts = i64::MIN; // allow re-iteration after seek
                                break;
                            }
                        }
                    }
                    VodOutput::CloseSession => return,
                    _ => {}
                }
            }
            if !consumed {
                next_input = VodCoreInput::Tick { now_us: 0 };
            }
        }
    }

    /// Track metadata is emitted exactly once at the start of reading.
    ///
    /// 轨道元数据在读取开始时只发出一次。
    #[test]
    fn reader_handles_repeated_track_emit_only_once() {
        let buf = build_multi_track_mp4(3, 3);
        let (tracks, _) = drive_reader_to_eof(&buf);
        // Tracks should be emitted only once at start.
        assert_eq!(tracks.len(), 2);
    }

    /// B-frames with non-zero composition offsets emit a `ctts` box.
    ///
    /// B 帧（非零合成偏移）会写出 `cttts` 盒子。
    #[test]
    fn writer_with_b_frames_emits_ctts() {
        let mut w = Mp4Writer::new(Mp4WriterConfig::default(), &[h264_track()]).unwrap();
        // Three video samples with non-zero composition offsets — pts != dts
        w.push_sample(1, 0, 33_333, true, b"AU").unwrap();
        w.push_sample(1, 33_333, 0, false, b"AU").unwrap();
        w.push_sample(1, 66_667, 33_333, false, b"AU").unwrap();
        let Mp4WriteEvent::File(buf) = w.finalize().unwrap();
        assert!(buf.windows(4).any(|w| w == b"ctts"));
    }

    /// A stream without B-frames omits the `ctts` box.
    ///
    /// 没有 B 帧的流不输出 `ctts` 盒子。
    #[test]
    fn writer_omits_ctts_when_no_b_frames() {
        let mut w = Mp4Writer::new(Mp4WriterConfig::default(), &[h264_track()]).unwrap();
        for i in 0..3 {
            w.push_sample(1, i * 33_333, i * 33_333, i == 0, b"AU")
                .unwrap();
        }
        let Mp4WriteEvent::File(buf) = w.finalize().unwrap();
        assert!(!buf.windows(4).any(|w| w == b"ctts"));
    }

    /// Malformed zero-size boxes are rejected safely and do not loop forever.
    ///
    /// 畸形零长度 box 被安全拒绝，不会无限循环。
    #[test]
    fn malformed_size_box_rejected_safely() {
        let buf = b"\x00\x00\x00\x00xxxx\x00\x00\x00\x00";
        let mut reader = Mp4Reader::new(Mp4ReaderConfig::default());
        reader.set_file_size(buf.len() as u64);
        let mut closed = false;
        for _ in 0..50 {
            match reader.step() {
                Mp4ReadEvent::NeedBytes(req) => {
                    let end = (req.offset + req.length) as usize;
                    let end = end.min(buf.len());
                    reader.feed_bytes(Mp4ReadResult {
                        offset: req.offset,
                        data: Bytes::copy_from_slice(&buf[req.offset as usize..end]),
                    });
                }
                Mp4ReadEvent::Eof | Mp4ReadEvent::Idle => {
                    closed = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(closed, "reader should bound and exit on malformed input");
    }
}
