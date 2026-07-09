#!/usr/bin/env bash
# Phase 06 GStreamer helper entrypoint.
#
# Usage:
#   cheetah-gst-helper whip   # publish synthetic VP8 to $WEBRTC_INTEROP_ZLM_WHIP_URL
#   cheetah-gst-helper whep   # play stream from $WEBRTC_INTEROP_ZLM_WHEP_URL
#
# Env:
#   WEBRTC_INTEROP_ZLM_WHIP_URL   WHIP endpoint URL (whip mode)
#   WEBRTC_INTEROP_ZLM_WHEP_URL   WHEP endpoint URL (whep mode)
#   WEBRTC_INTEROP_ARTIFACT_DIR   directory where peer.log is written
#   WEBRTC_INTEROP_TIMEOUT_MS     hard timeout in milliseconds (default 10000)

set -euo pipefail

mode="${1:-}"
if [[ -z "$mode" ]]; then
  echo "usage: $0 whip|whep" >&2
  exit 1
fi

artifact_dir="${WEBRTC_INTEROP_ARTIFACT_DIR:-/tmp/cheetah-gst}"
mkdir -p "$artifact_dir"
peer_log="$artifact_dir/peer.log"
timeout_seconds="$(( ${WEBRTC_INTEROP_TIMEOUT_MS:-10000} / 1000 ))"

run_with_timeout() {
  # `gst-launch-1.0 -e` triggers EOS on signal so SIGTERM is safe.
  timeout --signal=TERM "${timeout_seconds}" "$@" 2>&1 | tee "$peer_log" || true
}

case "$mode" in
  whip)
    : "${WEBRTC_INTEROP_ZLM_WHIP_URL:?missing WEBRTC_INTEROP_ZLM_WHIP_URL}"
    run_with_timeout gst-launch-1.0 -e \
      videotestsrc is-live=true pattern=ball ! \
      video/x-raw,width=320,height=240,framerate=30/1 ! \
      videoconvert ! vp8enc deadline=1 ! rtpvp8pay ! \
      "application/x-rtp,media=video,encoding-name=VP8,payload=96" ! \
      whipclientsink whip-endpoint-url="$WEBRTC_INTEROP_ZLM_WHIP_URL"
    ;;
  whep)
    : "${WEBRTC_INTEROP_ZLM_WHEP_URL:?missing WEBRTC_INTEROP_ZLM_WHEP_URL}"
    run_with_timeout gst-launch-1.0 -e \
      whepclientsrc whep-endpoint-url="$WEBRTC_INTEROP_ZLM_WHEP_URL" ! \
      rtpvp8depay ! vp8dec ! fakesink dump=false
    ;;
  *)
    echo "unknown mode: $mode (expected whip or whep)" >&2
    exit 1
    ;;
esac
