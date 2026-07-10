#!/usr/bin/env bash
# Start cheetah-server with tokio-console instrumentation and run tokio-console for a short
# capture. This requires the tokio_unstable cfg and the `tokio-console` feature.
# Usage: ./dev-scripts/perf/async_tokio_console.sh [duration-seconds]
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

DURATION="${1:-15}"
OUT_DIR="${OUT_DIR:-target/perf}"
mkdir -p "${OUT_DIR}"

if ! command -v tokio-console >/dev/null 2>&1; then
  echo "[async_tokio_console] tokio-console not found; install with: cargo install tokio-console" >&2
  exit 1
fi

echo "[async_tokio_console] building cheetah-server with tokio-console feature"
RUSTFLAGS="--cfg tokio_unstable" cargo build -p cheetah-server --features tokio-console

SERVER_BIN="${ROOT_DIR}/target/debug/cheetah-server"
OUT_LOG="${OUT_DIR}/tokio-console.log"

echo "[async_tokio_console] starting server (will run ${DURATION}s)"
RUSTFLAGS="--cfg tokio_unstable" RUST_LOG=info "${SERVER_BIN}" >"${OUT_DIR}/tokio-server.log" 2>&1 &
sleep 2

# Verify the console subscriber is listening.
if ! ss -ltn 2>/dev/null | grep -q ':6669'; then
  echo "[async_tokio_console] warning: console subscriber not listening on 127.0.0.1:6669" >&2
fi

cleanup() {
  pkill -f 'target/debug/cheetah-server$' 2>/dev/null || true
  # Wait briefly for the process to terminate.
  for _ in {1..5}; do
    if ! pgrep -f 'target/debug/cheetah-server$' >/dev/null 2>&1; then
      break
    fi
    sleep 0.2
  done
}
trap cleanup EXIT

echo "[async_tokio_console] running tokio-console for ${DURATION}s"
# tokio-console is an interactive TUI. In a headless environment, wrap it with
# `script` to provide a pseudo-TTY so the TUI can render.
if [ -t 1 ]; then
  timeout "${DURATION}" tokio-console 2>&1 | tee "${OUT_LOG}" || true
else
  TERM=xterm-256color script -q -e -c "timeout ${DURATION} tokio-console" "${OUT_LOG}" || true
fi

echo "[async_tokio_console] done: server stopped, log at ${OUT_LOG}"
