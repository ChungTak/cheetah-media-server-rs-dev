use std::io::Cursor;
use std::process::Command;
use std::sync::Arc;

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId,
    TrackInfo,
};
use cheetah_config::ConfigStore;
use cheetah_engine::{EngineBuilder, LocalFfmpegService};
use cheetah_media_api::error::MediaErrorCode;
use cheetah_media_api::image::{ImageEncodeApi, ImageEncodeRequest, ImageFormat};
use cheetah_media_api::port::MediaRequestContext;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_snapshot_module::{ImageEncoderBackend, SnapshotModuleFactory};
use serde_json::json;
use tokio::time::timeout;

fn make_jpeg_bytes(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbaImage::new(width, height);
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .expect("encode jpeg");
    buf.into_inner()
}

fn make_frame(payload: Vec<u8>) -> Arc<cheetah_codec::AVFrame> {
    Arc::new(AVFrame::new(
        TrackId(0),
        MediaKind::Video,
        CodecId::MJPEG,
        FrameFormat::MjpegFrame,
        0,
        0,
        Timebase::new(1, 1_000_000),
        Bytes::from(payload),
    ))
}

fn make_track() -> TrackInfo {
    TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::MJPEG, 90_000)
}

#[tokio::test]
async fn image_encoder_decodes_mjpeg_and_reencodes_to_jpeg() {
    let jpeg = make_jpeg_bytes(16, 12);
    let backend = ImageEncoderBackend::new();
    let artifact = backend
        .encode(
            &MediaRequestContext::default(),
            ImageEncodeRequest {
                frame: make_frame(jpeg),
                track_info: make_track(),
                format: ImageFormat::Jpeg,
                quality: 80,
                max_width: None,
                max_height: None,
            },
        )
        .await
        .expect("encode should succeed");

    assert_eq!(artifact.format, ImageFormat::Jpeg);
    assert_eq!(artifact.content_type, "image/jpeg");
    assert_eq!(artifact.width, 16);
    assert_eq!(artifact.height, 12);
    assert!(artifact.payload.starts_with(&[0xff, 0xd8]));
    assert!(!artifact.payload.is_empty());
}

#[tokio::test]
async fn image_encoder_scales_to_max_dimensions_preserving_aspect_ratio() {
    let jpeg = make_jpeg_bytes(64, 36);
    let backend = ImageEncoderBackend::new();
    let artifact = backend
        .encode(
            &MediaRequestContext::default(),
            ImageEncodeRequest {
                frame: make_frame(jpeg),
                track_info: make_track(),
                format: ImageFormat::Jpeg,
                quality: 50,
                max_width: Some(16),
                max_height: None,
            },
        )
        .await
        .expect("encode should succeed");

    assert_eq!(artifact.width, 16);
    assert_eq!(artifact.height, 9);
}

#[tokio::test]
async fn image_encoder_rejects_unsupported_video_codecs() {
    let frame = Arc::new(AVFrame::new(
        TrackId(0),
        MediaKind::Video,
        CodecId::H265,
        FrameFormat::CanonicalH26x,
        0,
        0,
        Timebase::new(1, 1_000_000),
        Bytes::from_static(b"fake-h265"),
    ));
    let track = TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H265, 90_000);
    let backend = ImageEncoderBackend::new();
    let err = backend
        .encode(
            &MediaRequestContext::default(),
            ImageEncodeRequest {
                frame,
                track_info: track,
                format: ImageFormat::Jpeg,
                quality: 90,
                max_width: None,
                max_height: None,
            },
        )
        .await
        .expect_err("unsupported video codec should fail");

    assert_eq!(err.code, MediaErrorCode::Unsupported);
}

#[tokio::test]
async fn image_encoder_rejects_h264_without_ffmpeg() {
    let frame = Arc::new(AVFrame::new(
        TrackId(0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        0,
        0,
        Timebase::new(1, 1_000_000),
        Bytes::from_static(b"fake-h264"),
    ));
    let track = TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H264, 90_000);
    let backend = ImageEncoderBackend::new();
    let err = backend
        .encode(
            &MediaRequestContext::default(),
            ImageEncodeRequest {
                frame,
                track_info: track,
                format: ImageFormat::Jpeg,
                quality: 90,
                max_width: None,
                max_height: None,
            },
        )
        .await
        .expect_err("H264 without ffmpeg should fail");

    assert_eq!(err.code, MediaErrorCode::Unavailable);
}

fn generate_h264_keyframe() -> (Bytes, Bytes, Bytes) {
    let output = Command::new("ffmpeg")
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=8x6:rate=1",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-frames:v",
            "1",
            "-f",
            "h264",
            "-",
        ])
        .output()
        .expect("spawn ffmpeg");
    assert!(
        output.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let nals = split_annex_b(&output.stdout);
    let mut sps = None;
    let mut pps = None;
    let mut idr = None;
    for nal in nals {
        if nal.is_empty() {
            continue;
        }
        let nal_type = nal[0] & 0x1f;
        match nal_type {
            7 if sps.is_none() => sps = Some(Bytes::from(nal)),
            8 if pps.is_none() => pps = Some(Bytes::from(nal)),
            5 if idr.is_none() => idr = Some(Bytes::from(nal)),
            _ => {}
        }
    }
    (
        sps.expect("missing SPS"),
        pps.expect("missing PPS"),
        idr.expect("missing IDR"),
    )
}

fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn split_annex_b(data: &[u8]) -> Vec<Vec<u8>> {
    let mut nals = Vec::new();
    let mut i = 0;
    while i + 2 < data.len() {
        let start_len = if data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && i + 3 < data.len()
            && data[i + 3] == 1
        {
            4
        } else if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            3
        } else {
            i += 1;
            continue;
        };

        let nal_start = i + start_len;
        let mut j = nal_start;
        while j + 2 < data.len() {
            if data[j] == 0
                && data[j + 1] == 0
                && data[j + 2] == 0
                && j + 3 < data.len()
                && data[j + 3] == 1
            {
                break;
            }
            if data[j] == 0 && data[j + 1] == 0 && data[j + 2] == 1 {
                break;
            }
            j += 1;
        }
        if j + 2 >= data.len() {
            j = data.len();
        }
        nals.push(data[nal_start..j].to_vec());
        i = nal_start;
    }
    nals
}

#[tokio::test]
async fn image_encoder_decodes_h264_keyframe_with_ffmpeg() {
    if !ffmpeg_available() {
        return;
    }
    let (sps, pps, idr) = generate_h264_keyframe();
    let mut frame = AVFrame::new(
        TrackId(0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        0,
        0,
        Timebase::new(1, 90_000),
        idr,
    );
    frame.flags = FrameFlags::KEY;

    let mut track = TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H264, 90_000);
    track.extradata = CodecExtradata::H264 {
        sps: vec![sps],
        pps: vec![pps],
        avcc: None,
    };

    let ffmpeg = Arc::new(LocalFfmpegService::new());
    let backend = ImageEncoderBackend::new().with_ffmpeg_api(ffmpeg);
    let artifact = backend
        .encode(
            &MediaRequestContext::default(),
            ImageEncodeRequest {
                frame: Arc::new(frame),
                track_info: track,
                format: ImageFormat::Jpeg,
                quality: 90,
                max_width: None,
                max_height: None,
            },
        )
        .await
        .expect("encode H264 keyframe");

    assert_eq!(artifact.format, ImageFormat::Jpeg);
    assert!(artifact.payload.starts_with(&[0xff, 0xd8]));
    assert!(artifact.width > 0);
    assert!(artifact.height > 0);
}

#[tokio::test]
async fn snapshot_module_registers_image_encode_provider() {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({}));
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .with_config_schema_registry(config)
        .register_module_factory(Arc::new(SnapshotModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let facade = engine.media_facade();
    let jpeg = make_jpeg_bytes(8, 6);
    let artifact = facade
        .encode(
            &MediaRequestContext::default(),
            ImageEncodeRequest {
                frame: make_frame(jpeg),
                track_info: make_track(),
                format: ImageFormat::Png,
                quality: 90,
                max_width: None,
                max_height: None,
            },
        )
        .await
        .expect("encode through engine facade");

    assert_eq!(artifact.format, ImageFormat::Png);
    assert_eq!(artifact.content_type, "image/png");

    let _ = timeout(std::time::Duration::from_secs(5), engine.stop()).await;
}
