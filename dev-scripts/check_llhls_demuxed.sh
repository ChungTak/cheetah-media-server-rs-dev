#!/usr/bin/env bash
# check_llhls_demuxed.sh — Verify demuxed LLHLS audio/video output.
#
# Prerequisites:
#   1. Server running: cargo run -p cheetah-server --features "rtsp,hls"
#   2. Push stream:    ffmpeg -stream_loop -1 -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv \
#                        -c copy -f flv rtmp://127.0.0.1:1935/live/test
#   3. curl and ffprobe available
#
# Usage: bash dev-scripts/check_llhls_demuxed.sh [BASE_URL] [STREAM]

set -euo pipefail

BASE="${1:-http://127.0.0.1:8088}"
STREAM="${2:-live/test}"
PASS=0
FAIL=0

check() {
  local desc="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    echo "  ✓ $desc"
    ((PASS++))
  else
    echo "  ✗ $desc"
    ((FAIL++))
  fi
}

echo "=== LLHLS Demuxed A/V Verification ==="
echo "Base: $BASE  Stream: $STREAM"
echo ""

# --- Master Playlist ---
echo "[Master Playlist]"
MASTER=$(curl -sf "$BASE/$STREAM.m3u8" || true)
if [ -z "$MASTER" ]; then
  echo "  ✗ Failed to fetch master playlist (is the server running with a stream?)"
  echo ""
  echo "RESULT: 0 passed, 1 failed"
  exit 1
fi

check "Contains EXT-X-MEDIA:TYPE=AUDIO" echo "$MASTER" | grep -q "#EXT-X-MEDIA:TYPE=AUDIO"
check "Contains AUDIO=\"audio\" group" echo "$MASTER" | grep -q 'AUDIO="audio"'
check "Contains chunklist_video.m3u8" echo "$MASTER" | grep -q "chunklist_video.m3u8"
check "Contains chunklist_audio.m3u8" echo "$MASTER" | grep -q "chunklist_audio.m3u8"
echo ""

# --- Extract session UID from master ---
UID=$(echo "$MASTER" | grep -o 'uid=[0-9]*' | head -1 | cut -d= -f2)
UID_PARAM=""
[ -n "$UID" ] && UID_PARAM="?uid=$UID"

# --- Video Chunklist ---
echo "[Video Chunklist]"
VIDEO_PL=$(curl -sf "$BASE/$STREAM/chunklist_video.m3u8${UID_PARAM}" || true)
if [ -n "$VIDEO_PL" ]; then
  check "Has EXT-X-MAP with init_video.mp4" echo "$VIDEO_PL" | grep -q 'EXT-X-MAP:URI="init_video.mp4'
  check "Has video_part_ entries" echo "$VIDEO_PL" | grep -q "video_part_"
  check "Has EXT-X-RENDITION-REPORT" echo "$VIDEO_PL" | grep -q "EXT-X-RENDITION-REPORT"
else
  echo "  ✗ Failed to fetch video chunklist"
  ((FAIL++))
fi
echo ""

# --- Audio Chunklist ---
echo "[Audio Chunklist]"
AUDIO_PL=$(curl -sf "$BASE/$STREAM/chunklist_audio.m3u8${UID_PARAM}" || true)
if [ -n "$AUDIO_PL" ]; then
  check "Has EXT-X-MAP with init_audio.mp4" echo "$AUDIO_PL" | grep -q 'EXT-X-MAP:URI="init_audio.mp4'
  check "Has audio_part_ entries" echo "$AUDIO_PL" | grep -q "audio_part_"
  check "Has EXT-X-RENDITION-REPORT" echo "$AUDIO_PL" | grep -q "EXT-X-RENDITION-REPORT"
else
  echo "  ✗ Failed to fetch audio chunklist"
  ((FAIL++))
fi
echo ""

# --- Init Segments (ffprobe) ---
echo "[Init Segments]"
if command -v ffprobe &>/dev/null; then
  curl -sf -o /tmp/cheetah_init_video.mp4 "$BASE/$STREAM/init_video.mp4${UID_PARAM}" 2>/dev/null || true
  curl -sf -o /tmp/cheetah_init_audio.mp4 "$BASE/$STREAM/init_audio.mp4${UID_PARAM}" 2>/dev/null || true

  if [ -s /tmp/cheetah_init_video.mp4 ]; then
    check "init_video.mp4 has video stream" ffprobe -v error -show_streams /tmp/cheetah_init_video.mp4 | grep -q "codec_type=video"
    check "init_video.mp4 has NO audio stream" ! ffprobe -v error -show_streams /tmp/cheetah_init_video.mp4 | grep -q "codec_type=audio"
  else
    echo "  ✗ init_video.mp4 not available"
    ((FAIL++))
  fi

  if [ -s /tmp/cheetah_init_audio.mp4 ]; then
    check "init_audio.mp4 has audio stream" ffprobe -v error -show_streams /tmp/cheetah_init_audio.mp4 | grep -q "codec_type=audio"
    check "init_audio.mp4 has NO video stream" ! ffprobe -v error -show_streams /tmp/cheetah_init_audio.mp4 | grep -q "codec_type=video"
  else
    echo "  ✗ init_audio.mp4 not available"
    ((FAIL++))
  fi
else
  echo "  (skipped: ffprobe not found)"
fi
echo ""

# --- Summary ---
TOTAL=$((PASS + FAIL))
echo "=== RESULT: $PASS passed, $FAIL failed (of $TOTAL checks) ==="
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
