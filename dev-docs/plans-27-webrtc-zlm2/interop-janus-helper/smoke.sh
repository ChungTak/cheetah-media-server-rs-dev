#!/usr/bin/env bash
# Phase 06 Janus smoke driver. Performs the minimal three-step
# Janus REST handshake (create session, attach echotest plugin,
# send a noop message) and writes the responses to the artifact
# directory so the cheetah harness can verify them.
#
# Usage:
#   janus-smoke
#
# Env:
#   WEBRTC_INTEROP_JANUS_URL    Janus REST endpoint (default http://127.0.0.1:8088/janus)
#   WEBRTC_INTEROP_ARTIFACT_DIR directory for response logs (default /tmp/cheetah-janus)

set -euo pipefail

url="${WEBRTC_INTEROP_JANUS_URL:-http://127.0.0.1:8088/janus}"
artifact_dir="${WEBRTC_INTEROP_ARTIFACT_DIR:-/tmp/cheetah-janus}"
mkdir -p "$artifact_dir"

txn() { tr -dc 'a-zA-Z0-9' </dev/urandom | head -c 12; }

step1=$(curl -fsS -X POST "$url" \
  -H 'Content-Type: application/json' \
  -d "{\"janus\":\"create\",\"transaction\":\"$(txn)\"}")
echo "$step1" >"$artifact_dir/step1-create.json"
session_id=$(echo "$step1" | jq -r '.data.id')
if [[ -z "$session_id" || "$session_id" == "null" ]]; then
  echo "create failed: $step1" >&2
  exit 1
fi

step2=$(curl -fsS -X POST "$url/$session_id" \
  -H 'Content-Type: application/json' \
  -d "{\"janus\":\"attach\",\"plugin\":\"janus.plugin.echotest\",\"transaction\":\"$(txn)\"}")
echo "$step2" >"$artifact_dir/step2-attach.json"
handle_id=$(echo "$step2" | jq -r '.data.id')
if [[ -z "$handle_id" || "$handle_id" == "null" ]]; then
  echo "attach failed: $step2" >&2
  exit 1
fi

step3=$(curl -fsS -X POST "$url/$session_id/$handle_id" \
  -H 'Content-Type: application/json' \
  -d "{\"janus\":\"message\",\"body\":{\"video\":true,\"audio\":true,\"bitrate\":64000},\"transaction\":\"$(txn)\"}")
echo "$step3" >"$artifact_dir/step3-message.json"

echo "OK session=$session_id handle=$handle_id"
