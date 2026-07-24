#!/usr/bin/env bash
# 24 h RTP self-loop soak harness.
# Usage: scripts/rtp-24h-soak.sh [duration_hours]
set -euo pipefail

DURATION_HOURS="${1:-24}"
DURATION_SECS=$((DURATION_HOURS * 3600))

COMMIT=$(git rev-parse --short HEAD)
DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ)
LOG_DIR="soak-logs"
mkdir -p "$LOG_DIR"
LOG_FILE="$LOG_DIR/rtp-soak-${COMMIT}-${DATE}.log"

echo "Starting ${DURATION_HOURS}h RTP soak (commit ${COMMIT} at ${DATE})" | tee "$LOG_FILE"
echo "SOAK_DURATION_SECS=${DURATION_SECS} SOAK_SEND_INTERVAL_MS=20" | tee -a "$LOG_FILE"

SOAK_DURATION_SECS="$DURATION_SECS" \
SOAK_SEND_INTERVAL_MS=20 \
cargo test -p cheetah-rtp-driver-tokio --test soak -- rtp_send_self_loop_soak --ignored --nocapture 2>&1 | tee -a "$LOG_FILE"

echo "Soak log written to $LOG_FILE"
