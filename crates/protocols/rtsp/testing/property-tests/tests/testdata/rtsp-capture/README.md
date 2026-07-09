# RTSP Capture Fixture Test Data

This directory stores compact RTSP capture fixtures extracted from real
pcap files under `test_media_files/dump_rtsp_sms_gst`.

The original `test_media_files` directory is ignored by git. Tests must consume
committed `.rtspcap` fixtures in this directory and must not read source pcap
files directly.

## Layout

```text
manifest.tsv
skipped.tsv
standard/
probes/
```

- `standard/` contains fixtures used for strong assertions in core/module/property-tests
  tests, for example RTSP method sequence and minimal RTP parsing.
- `probes/` contains compatibility and robustness fixtures. Probe inputs are
  used for bounded robustness checks and do not have to play successfully.

## `.rtspcap` Format

Each `.rtspcap` file contains only extracted transport payload records. It does
not store IP/TCP/UDP headers, NIC metadata, or full pcap records.

```text
magic: 4 bytes = "RSF1"
record_count: u32 big-endian
records:
  kind: u8
  flags: u8
  flow_id: u16 big-endian
  delta_us: u32 big-endian
  payload_len: u32 big-endian
  payload_bytes: [payload_len]
```

`kind` covers RTSP TCP C2S/S2C payload, TCP interleaved RTP/RTCP payload, and
UDP RTP/RTCP datagrams.

## Manifest

`manifest.tsv` has a fixed header:

```text
case	source_pcap	stream_name	media_sig	push_transport	pull_transport	role	fixture	expect_methods	expect_rtp_min	expect_rtcp_min	expect_tracks_min	notes
```

Rows are added after fixture extraction. Manifest paths must stay inside this
directory tree.

`push_transport` / `pull_transport` values:

```text
tcp
udp
http-tunnel
multicast
mixed
none
```

`role` values:

```text
standard_publish_tcp
standard_publish_udp
standard_publish_http_tunnel
standard_play_tcp
standard_play_udp
standard_play_multicast
standard_pull_job
standard_push_job
standard_relay_job
compat_probe
transport_fault_seed
```

`skipped.tsv` records sources that were intentionally skipped during extraction.
The `reason` column uses stable reason codes such as `skipped_empty_pcap`.

The short-name matrix pcap files such as `h264_aac__push_tcp__pull_tcp.pcap`
are currently empty in local capture sets. Initial committed fixtures must be
extracted from non-empty `from_file_*` captures; empty pcap files are skipped
and are not CI inputs.

## Regeneration

```bash
python3 dev-scripts/rtsp_extract_capture_fixtures.py \
  --source-dir test_media_files/dump_rtsp_sms_gst \
  --out-dir crates/protocols/rtsp/testing/property-tests/tests/testdata/rtsp-capture \
  --max-fixture-bytes 524288
```

The extractor must skip empty or malformed pcap files and must not generate
empty fixtures for skipped inputs.
