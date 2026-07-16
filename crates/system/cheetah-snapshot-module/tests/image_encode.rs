use std::io::Cursor;
use std::sync::Arc;

use bytes::Bytes;
use cheetah_codec::{AVFrame, CodecId, FrameFormat, MediaKind, Timebase, TrackId, TrackInfo};
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
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
async fn image_encoder_rejects_non_mjpeg_codecs() {
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
        .expect_err("non-mjpeg should fail");

    assert_eq!(err.code, MediaErrorCode::Unsupported);
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
