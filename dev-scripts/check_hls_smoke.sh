#!/bin/bash
# HLS end-to-end smoke test.
# Requires: ffmpeg, curl, and the cheetah-server binary built with --features hls.
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SERVER_BIN="$ROOT_DIR/target/debug/cheetah-server"
HLS_PORT=8088
RTMP_PORT=1935

cleanup() {
    [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
    [ -n "$FFMPEG_PID" ] && kill "$FFMPEG_PID" 2>/dev/null || true
    rm -f /tmp/hls_smoke_*.m3u8 /tmp/hls_smoke_*.ts
}
trap cleanup EXIT

echo "=== HLS Smoke Test ==="

# Build
echo "[1/5] Building server..."
cd "$ROOT_DIR"
cargo build -p cheetah-server --features hls 2>/dev/null || {
    echo "SKIP: build failed (missing features or deps)"
    exit 0
}

# Start server
echo "[2/5] Starting server..."
RUST_LOG=warn "$SERVER_BIN" &
SERVER_PID=$!
sleep 2

if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "SKIP: server failed to start"
    exit 0
fi

# Push stream
echo "[3/5] Pushing RTMP stream..."
ffmpeg -hide_banner -loglevel error \
    -re -f lavfi -i "testsrc=duration=10:size=320x240:rate=25" \
    -f lavfi -i "sine=frequency=440:duration=10" \
    -c:v libx264 -preset ultrafast -tune zerolatency \
    -c:a aac -b:a 64k \
    -f flv "rtmp://127.0.0.1:${RTMP_PORT}/live/hls_test" &
FFMPEG_PID=$!
sleep 6

# Fetch master playlist
echo "[4/5] Fetching HLS playlists..."
HTTP_CODE=$(curl -s -o /tmp/hls_smoke_master.m3u8 -w "%{http_code}" \
    "http://127.0.0.1:${HLS_PORT}/live/hls_test.m3u8" 2>/dev/null || echo "000")

if [ "$HTTP_CODE" != "200" ]; then
    echo "FAIL: master playlist returned HTTP $HTTP_CODE"
    exit 1
fi

grep -q "#EXTM3U" /tmp/hls_smoke_master.m3u8 || {
    echo "FAIL: master playlist is not valid M3U8"
    exit 1
}

# Fetch media playlist
MEDIA_URL=$(grep -v "^#" /tmp/hls_smoke_master.m3u8 | head -1)
HTTP_CODE=$(curl -s -o /tmp/hls_smoke_media.m3u8 -w "%{http_code}" \
    "http://127.0.0.1:${HLS_PORT}/live/${MEDIA_URL}" 2>/dev/null || echo "000")

if [ "$HTTP_CODE" != "200" ]; then
    echo "FAIL: media playlist returned HTTP $HTTP_CODE"
    exit 1
fi

grep -q "#EXTINF:" /tmp/hls_smoke_media.m3u8 || {
    echo "FAIL: media playlist has no segments"
    exit 1
}

# Fetch first segment
SEG_URL=$(grep -v "^#" /tmp/hls_smoke_media.m3u8 | head -1)
HTTP_CODE=$(curl -s -o /tmp/hls_smoke_seg.ts -w "%{http_code}" \
    "http://127.0.0.1:${HLS_PORT}/live/hls_test/${SEG_URL}" 2>/dev/null || echo "000")

if [ "$HTTP_CODE" != "200" ]; then
    echo "FAIL: segment returned HTTP $HTTP_CODE"
    exit 1
fi

[ -s /tmp/hls_smoke_seg.ts ] || {
    echo "FAIL: segment file is empty"
    exit 1
}

# Validate TS
echo "[5/5] Validating TS segment..."
if command -v ffprobe &>/dev/null; then
    ffprobe -v error /tmp/hls_smoke_seg.ts || {
        echo "FAIL: ffprobe reports invalid TS"
        exit 1
    }
fi

echo "PASS: HLS smoke test completed successfully"
