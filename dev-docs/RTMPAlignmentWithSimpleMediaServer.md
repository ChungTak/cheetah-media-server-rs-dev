# RTMP Alignment With SimpleMediaServer

This document records the current RTMP alignment status against `vendor-ref/simple-media-server`
based on packet captures and logs under `test_media_files/dump_rtmp/`.

## Evidence Inputs

- `test_media_files/dump_rtmp/summary_from_files.tsv`
- `test_media_files/dump_rtmp/summary.tsv`
- `test_media_files/dump_rtmp/*.pcap`
- `test_media_files/dump_rtmp/simplemediaserver_from_files.log`

## Alignment Categories

### 1) SMS-aligned (wire behavior proven by capture)

- `h264_aac_from_file`
- `h265_aac_from_file`
- `av1_aac_from_file`
- `vp9_aac_from_file`
- `aac_only_from_file`
- `mp3_only_from_file`
- `g711a_only_from_file`
- `g711u_only_from_file`
- `adpcm_only_from_file`

These paths have successful push+pull and observed SMS-compatible control-plane ordering
(`connect/createStream` results, play status sequence, and `|RtmpSampleAccess`) plus expected media headers.

### 2) Pending evidence (source code exists, capture proof not complete)

- `h266_aac_from_file`
- `vp8_aac_from_file`

Current captures fail before complete publish/play media path is established, so these are not counted
as aligned yet even if code paths exist.

### 3) Extension, not SMS-aligned baseline

- `opus_only_from_file`

Current baseline treats Opus-over-RTMP as Rust extension capability. In SMS captures, publish starts
but stream readiness/play path does not complete.

## Non-standard Compatibility Points Implemented

- Enhanced play mode query: `type=enhanced` and `type=fastPts`.
- Enhanced video signaling with fourcc for `hvc1`, `av01`, `vp08`, `vp09`, `vvc1`.
- VPX/AV1 enhanced ingress tolerance for packet type 1 without CTS bytes.
- AV1 enhanced config vendor prefix handling (`81 ff ff ff`).

## Verification Script

Run the baseline verifier:

```bash
test_media_files/dump_rtmp/validate_alignment_from_files.sh
```

The script checks aligned cases, pending-evidence cases, and the Opus extension classification
against `summary_from_files.tsv` and SMS logs.
