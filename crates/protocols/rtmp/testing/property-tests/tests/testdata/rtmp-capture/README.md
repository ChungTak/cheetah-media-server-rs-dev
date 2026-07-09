# RTMP Capture Fixture Test Data

This directory stores compact RTMP byte-stream fixtures extracted from real
pcap captures under `test_media_files/dump_rtmp_sms_gst`.

The original `test_media_files` directory is ignored by git, and many source
pcap files may be empty in a local checkout. Tests must therefore consume the
committed `.rtmpflow` fixtures in this directory rather than reading source
pcap files directly.

## Layout

```text
manifest.tsv
standard/
probes/
```

- `standard/` stores fixtures expected to satisfy normal RTMP protocol
  assertions, such as `Connected`, `PublishRequested`, `MediaData`, and
  monotonic media timestamps.
- `probes/` stores enhanced, fallback, malformed, truncated, or compatibility
  samples that are used for bounded robustness checks. Probe inputs do not have
  to decode or play successfully.

## `.rtmpflow` Format

Each `.rtmpflow` file contains only TCP payload bytes. It does not store IP/TCP
headers, packet timestamps, interface metadata, or pcap records.

```text
magic: 4 bytes = "CRF1"
record_count: u32 big-endian
records:
  payload_len: u32 big-endian
  payload_bytes: [payload_len]
```

Each record is one TCP payload segment from the selected RTMP flow. Standard
fixtures are truncated only at record boundaries. Tests may dynamically create
half-packet, sticky-packet, dropped-record, duplicated-record, or reordered
views from the committed records.

## Manifest

`manifest.tsv` has a fixed header:

```text
case	source_pcap	stream_name	media_sig	role	fixture	expect_connected	expect_publish	expect_play	expect_media_min	notes
```

Rows are added when `.rtmpflow` files are generated. The manifest must not
refer to files outside this directory.

The short names such as `h264_aac.pcap`, `av1_aac.pcap`, `vp8_aac.pcap`,
`vp9_aac.pcap`, and `h266_aac.pcap` are empty in the current local capture
set. The committed fixtures are generated from the non-empty `from_file_*`
captures listed in `manifest.tsv`; empty source pcaps are not CI inputs.

## Regeneration

After the extractor is implemented, fixtures are regenerated with:

```bash
python3 dev-scripts/rtmp_extract_capture_fixtures.py \
  --source-dir test_media_files/dump_rtmp_sms_gst \
  --out-dir crates/protocols/rtmp/testing/property-tests/tests/testdata/rtmp-capture \
  --max-fixture-bytes 262144
```

The extractor must skip empty pcaps and malformed pcaps. It must not create
empty fixtures for skipped inputs.
