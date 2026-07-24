#!/usr/bin/env bash
# Run the RTP/codec micro-benchmarks and write a timestamped baseline report.
# Usage: scripts/rtp-perf-baseline.sh [output_dir]
set -euo pipefail

OUT_DIR="${1:-perf-baseline}"
mkdir -p "$OUT_DIR"

COMMIT=$(git rev-parse --short HEAD)
DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ)
OUT_FILE="$OUT_DIR/rtp-perf-baseline-${COMMIT}-${DATE}.txt"

echo "Recording baseline for commit $COMMIT at $DATE" | tee "$OUT_FILE"
echo "Rust toolchain:" >> "$OUT_FILE"
rustc --version >> "$OUT_FILE"
cargo --version >> "$OUT_FILE"
echo "" >> "$OUT_FILE"

echo "Running cargo bench -p cheetah-codec --bench codec_bench" | tee -a "$OUT_FILE"
cargo bench -p cheetah-codec --bench codec_bench -- --noplot 2>&1 | tee -a "$OUT_FILE"

echo "Baseline written to $OUT_FILE"
