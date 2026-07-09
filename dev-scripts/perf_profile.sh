#!/bin/bash
# Performance profiling script for cheetah-media-server
# Usage: ./scripts/perf_profile.sh [duration_seconds]

DURATION=${1:-30}
SERVER_BIN="./build/install/bin/cheetah-media-server"
PERF_DATA="perf.data"

echo "=== Cheetah Media Server Performance Profiler ==="
echo ""

# Check if perf is available
if ! command -v perf &> /dev/null; then
    echo "Error: perf is not installed"
    echo "Install with: sudo apt-get install linux-tools-common linux-tools-generic linux-tools-\$(uname -r)"
    exit 1
fi

# Check if server binary exists
if [ ! -f "$SERVER_BIN" ]; then
    echo "Error: $SERVER_BIN not found"
    echo "Please build the project first."
    exit 1
fi

# Check if server is already running
PID=$(pidof cheetah-media-server 2>/dev/null)
if [ -n "$PID" ]; then
    echo "Server already running with PID: $PID"
    echo "Attaching perf to existing process..."
    echo ""
    echo ">>> Start your 1000-client stress test NOW <<<"
    echo ">>> Recording for $DURATION seconds... <<<"
    echo ""
    sudo perf record -F 99 -g -p $PID -- sleep $DURATION
else
    echo "Starting server and recording..."
    echo ""
    echo ">>> Start your 1000-client stress test after server starts <<<"
    echo ">>> Recording for $DURATION seconds... <<<"
    echo ""
    sudo perf record -F 99 -g -- timeout $DURATION $SERVER_BIN
fi

echo ""
echo "=== Generating Report ==="
echo ""

# Generate text report
echo "Top 30 hottest functions:"
echo "========================="
sudo perf report --stdio --no-children -g none --percent-limit 0.5 2>/dev/null | head -50

echo ""
echo "=== Call Graph (top functions with callers) ==="
sudo perf report --stdio --no-children -g caller --percent-limit 1 2>/dev/null | head -100

echo ""
echo "=== Saving detailed report to perf_report.txt ==="
sudo perf report --stdio > perf_report.txt 2>/dev/null

echo ""
echo "Done! For interactive analysis, run: sudo perf report"
echo "Flame graph: perf script | stackcollapse-perf.pl | flamegraph.pl > flame.svg"
