mod support;

use std::path::Path;

use cheetah_codec::{FlvDemuxEvent, FlvDemuxer, FlvTagType};

#[test]
fn manifest_paths_are_safe_and_fixtures_are_bounded_parseable() {
    let cases = support::load_manifest_cases();
    let root = support::testdata_root();
    assert!(!cases.is_empty(), "manifest should not be empty");

    for case in cases {
        assert!(
            matches!(
                case.role.as_str(),
                "standard_play" | "standard_pull" | "compat_probe" | "transport_fault_seed"
            ),
            "invalid role for case {}: {}",
            case.case_name,
            case.role
        );
        assert!(!case.source.trim().is_empty(), "source must not be empty");
        assert!(
            !case.media_sig.trim().is_empty(),
            "media_sig must not be empty"
        );
        assert!(!case.notes.trim().is_empty(), "notes must not be empty");

        let rel = Path::new(case.fixture.as_str());
        assert!(
            rel.components()
                .all(|component| !matches!(component, std::path::Component::ParentDir)),
            "fixture path must not contain parent dir: {}",
            case.fixture
        );

        let path = support::fixture_path(&case.fixture);
        assert!(
            path.starts_with(&root),
            "fixture path must stay inside testdata root"
        );
        let bytes = support::load_fixture_bytes(&case.fixture);
        assert!(
            bytes.len() <= 64 * 1024,
            "fixture {} is too large: {} bytes",
            case.fixture,
            bytes.len()
        );

        let mut demuxer = FlvDemuxer::default();
        let events = demuxer.push(&bytes).expect("demux fixture");
        let mut header_count = 0usize;
        let mut metadata_count = 0usize;
        let mut video_count = 0usize;
        let mut audio_count = 0usize;
        for event in events {
            match event {
                FlvDemuxEvent::Header(_) => header_count += 1,
                FlvDemuxEvent::Tag(tag) => match tag.tag_type {
                    FlvTagType::Script => metadata_count += 1,
                    FlvTagType::Video => video_count += 1,
                    FlvTagType::Audio => audio_count += 1,
                },
                FlvDemuxEvent::PreviousTagSizeMismatch(_) => {}
            }
        }

        if case.expect_header {
            assert_eq!(header_count, 1, "expected exactly one FLV header event");
        }
        assert!(
            metadata_count >= case.expect_metadata,
            "metadata count mismatch for {}",
            case.case_name
        );
        assert!(
            video_count >= case.expect_video_min,
            "video count mismatch for {}",
            case.case_name
        );
        assert!(
            audio_count >= case.expect_audio_min,
            "audio count mismatch for {}",
            case.case_name
        );
    }
}
