#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub mod fault_views;

#[derive(Debug, Clone)]
pub struct FixtureCase {
    pub case_name: String,
    pub source: String,
    pub media_sig: String,
    pub role: String,
    pub fixture: String,
    pub expect_header: bool,
    pub expect_metadata: usize,
    pub expect_video_min: usize,
    pub expect_audio_min: usize,
    pub notes: String,
}

pub fn testdata_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("testdata")
        .join("http-flv")
}

pub fn load_manifest_cases() -> Vec<FixtureCase> {
    let manifest = std::fs::read_to_string(testdata_root().join("manifest.tsv"))
        .expect("read http-flv manifest.tsv");
    let mut out = Vec::new();
    for (index, line) in manifest.lines().enumerate() {
        if index == 0 || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.len(),
            10,
            "manifest line {} must have 10 columns",
            index + 1
        );
        out.push(FixtureCase {
            case_name: cols[0].to_string(),
            source: cols[1].to_string(),
            media_sig: cols[2].to_string(),
            role: cols[3].to_string(),
            fixture: cols[4].to_string(),
            expect_header: parse_bool(cols[5], index),
            expect_metadata: parse_usize(cols[6], index, "expect_metadata"),
            expect_video_min: parse_usize(cols[7], index, "expect_video_min"),
            expect_audio_min: parse_usize(cols[8], index, "expect_audio_min"),
            notes: cols[9].to_string(),
        });
    }
    out
}

pub fn fixture_path(relative: &str) -> PathBuf {
    testdata_root().join(relative)
}

pub fn load_fixture_bytes(relative: &str) -> Vec<u8> {
    std::fs::read(fixture_path(relative)).expect("read .flvstream fixture")
}

fn parse_bool(raw: &str, line: usize) -> bool {
    match raw.trim() {
        "true" => true,
        "false" => false,
        other => panic!("manifest line {} invalid bool: {}", line + 1, other),
    }
}

fn parse_usize(raw: &str, line: usize, field: &str) -> usize {
    raw.trim()
        .parse::<usize>()
        .unwrap_or_else(|_| panic!("manifest line {} invalid {}: {}", line + 1, field, raw))
}
