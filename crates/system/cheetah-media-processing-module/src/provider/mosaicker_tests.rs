use crate::config::MediaProcessingModuleConfig;
use crate::provider::mosaicker::VideoMosaicker;
use avcodec::core::{BitstreamFormat, CodecId as AvCodecId, ImageInfo, Packet, Poll, TimeBase};
use avcodec::{VideoDecoderRequest, VideoProfile, VideoSdk};
use cheetah_codec::track::{MediaKind, TrackReadiness};
use cheetah_codec::{CodecId as CheetahCodecId, FrameFormat, Rational32, TrackId, TrackInfo};
use cheetah_media_api::processing::{MosaicCell, MosaicLayout, VideoMosaicInput};

#[test]
fn mosaicker_rejects_too_few_sources() {
    let config = MediaProcessingModuleConfig {
        profile: "software".to_string(),
        ..Default::default()
    };
    let layout = MosaicLayout {
        columns: 1,
        rows: 1,
        cell_width: 320,
        cell_height: 240,
        background: None,
        frame_rate_num: None,
        frame_rate_den: None,
        bit_rate: None,
        gop_size: None,
        video_codec: None,
        fit: None,
    };
    let input = VideoMosaicInput {
        source: cheetah_media_api::ids::MediaKey::new("_", "app", "s1", None).unwrap(),
        cell: MosaicCell {
            column: 0,
            row: 0,
            z_order: 0,
        },
        audio_gain_db: None,
        fit: None,
        label: None,
    };
    let track = TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30);
    let result = VideoMosaicker::new(&config, &[input], &layout, &[track]);
    assert!(result.is_err());
}

#[test]
fn mosaicker_rejects_odd_cell_dimensions() {
    let config = MediaProcessingModuleConfig {
        profile: "software".to_string(),
        ..Default::default()
    };
    let layout = MosaicLayout {
        columns: 2,
        rows: 1,
        cell_width: 321,
        cell_height: 240,
        background: None,
        frame_rate_num: None,
        frame_rate_den: None,
        bit_rate: None,
        gop_size: None,
        video_codec: None,
        fit: None,
    };
    let inputs = vec![
        VideoMosaicInput {
            source: cheetah_media_api::ids::MediaKey::new("_", "app", "s1", None).unwrap(),
            cell: MosaicCell {
                column: 0,
                row: 0,
                z_order: 0,
            },
            audio_gain_db: None,
            fit: None,
            label: None,
        },
        VideoMosaicInput {
            source: cheetah_media_api::ids::MediaKey::new("_", "app", "s2", None).unwrap(),
            cell: MosaicCell {
                column: 1,
                row: 0,
                z_order: 0,
            },
            audio_gain_db: None,
            fit: None,
            label: None,
        },
    ];
    let tracks = vec![
        TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
        TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
    ];
    let result = VideoMosaicker::new(&config, &inputs, &layout, &tracks);
    assert!(result.is_err());
}

#[test]
#[cfg(feature = "media-processing-cpu")]
fn mosaicker_produces_h264_output_from_black_canvas() {
    let config = MediaProcessingModuleConfig {
        profile: "software".to_string(),
        ..Default::default()
    };
    let layout = MosaicLayout {
        columns: 2,
        rows: 1,
        cell_width: 320,
        cell_height: 240,
        background: None,
        frame_rate_num: None,
        frame_rate_den: None,
        bit_rate: None,
        gop_size: None,
        video_codec: None,
        fit: None,
    };
    let inputs = vec![
        VideoMosaicInput {
            source: cheetah_media_api::ids::MediaKey::new("_", "app", "s1", None).unwrap(),
            cell: MosaicCell {
                column: 0,
                row: 0,
                z_order: 0,
            },
            audio_gain_db: None,
            fit: None,
            label: None,
        },
        VideoMosaicInput {
            source: cheetah_media_api::ids::MediaKey::new("_", "app", "s2", None).unwrap(),
            cell: MosaicCell {
                column: 1,
                row: 0,
                z_order: 0,
            },
            audio_gain_db: None,
            fit: None,
            label: None,
        },
    ];
    let mut tracks = vec![
        TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
        TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
    ];
    for track in &mut tracks {
        track.readiness = TrackReadiness::Ready;
        track.width = Some(640);
        track.height = Some(480);
        track.fps = Some(Rational32::new(30, 1));
    }

    let mut mosaicker = VideoMosaicker::new(&config, &inputs, &layout, &tracks).unwrap();
    let mut all_frames = Vec::new();
    for _ in 0..3 {
        all_frames.extend(mosaicker.tick().unwrap());
    }
    all_frames.extend(mosaicker.flush().unwrap());

    assert!(
        !all_frames.is_empty(),
        "mosaic should produce at least one encoded frame"
    );
    for frame in &all_frames {
        assert_eq!(frame.codec, CheetahCodecId::H264);
        assert_eq!(frame.media_kind, MediaKind::Video);
        assert_eq!(frame.format, FrameFormat::CanonicalH26x);
    }
}

#[test]
#[cfg(feature = "media-processing-cpu")]
fn mosaicker_output_decodes_back_to_image() {
    let config = MediaProcessingModuleConfig {
        profile: "software".to_string(),
        ..Default::default()
    };
    let layout = MosaicLayout {
        columns: 1,
        rows: 2,
        cell_width: 160,
        cell_height: 120,
        background: None,
        frame_rate_num: Some(30),
        frame_rate_den: Some(1),
        bit_rate: None,
        gop_size: None,
        video_codec: None,
        fit: None,
    };
    let inputs = vec![
        VideoMosaicInput {
            source: cheetah_media_api::ids::MediaKey::new("_", "app", "s1", None).unwrap(),
            cell: MosaicCell {
                column: 0,
                row: 0,
                z_order: 0,
            },
            audio_gain_db: None,
            fit: None,
            label: None,
        },
        VideoMosaicInput {
            source: cheetah_media_api::ids::MediaKey::new("_", "app", "s2", None).unwrap(),
            cell: MosaicCell {
                column: 0,
                row: 1,
                z_order: 0,
            },
            audio_gain_db: None,
            fit: None,
            label: None,
        },
    ];
    let mut tracks = vec![
        TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
        TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
    ];
    for track in &mut tracks {
        track.readiness = TrackReadiness::Ready;
        track.width = Some(160);
        track.height = Some(120);
        track.fps = Some(Rational32::new(30, 1));
    }

    let mut mosaicker = VideoMosaicker::new(&config, &inputs, &layout, &tracks).unwrap();
    let mut all_frames = Vec::new();
    for _ in 0..3 {
        all_frames.extend(mosaicker.tick().unwrap());
    }
    all_frames.extend(mosaicker.flush().unwrap());

    assert!(
        !all_frames.is_empty(),
        "mosaic should produce encoded frames"
    );

    let output_width = layout.columns * layout.cell_width;
    let output_height = layout.rows * layout.cell_height;

    let sdk = VideoSdk::new().expect("video sdk");
    let mut decoder = sdk
        .create_decoder(
            VideoProfile::Software,
            VideoDecoderRequest::new(AvCodecId::H264, TimeBase::new(1, 30)).unwrap(),
        )
        .expect("create h264 decoder")
        .into_session();

    for frame in &all_frames {
        let mut packet = Packet::from_host_bytes(
            avcodec::core::utils::next_buffer_id(),
            AvCodecId::H264,
            BitstreamFormat::H264AnnexB,
            frame.payload.to_vec(),
        );
        packet.pts = Some(frame.pts);
        packet.dts = Some(frame.dts);
        packet.time_base = Some(TimeBase::new(frame.timebase.num, frame.timebase.den));
        decoder.submit_packet(packet).expect("submit mosaic packet");

        for _ in 0..5 {
            match decoder.poll_image().expect("poll decoded image") {
                Poll::Ready(img) => {
                    assert_eq!(img.format, ImageInfo::Yuv420p);
                    assert_eq!(img.visible.width, output_width);
                    assert_eq!(img.visible.height, output_height);
                    return;
                }
                Poll::Pending => {}
                Poll::EndOfStream => break,
            }
        }
    }

    panic!("did not decode any mosaic output frame");
}
