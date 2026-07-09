#!/bin/bash
# Valgrind Memory Leak Check Script for Cheetah Media Server
# Usage: ./scripts/valgrind_check.sh [duration_in_seconds]

DURATION=${1:-90}
BIN_PATH="./build/install/bin/cheetah-media-server"
LOG_FILE="valgrind_report.txt"

# Ensure the debug binary exists
if [ ! -f "$BIN_PATH" ]; then
    echo "Debug binary not found at $BIN_PATH"
    echo "Please build the project first."
    exit 1
fi

echo "Starting Valgrind check on $BIN_PATH..."
echo "Server will run for $DURATION seconds."
echo "Please perform RTMP push/pull operations during this time."
echo "Logs will be saved to $LOG_FILE"
echo "---------------------------------------------------"

# Run Valgrind in the background
# --leak-check=full: Detailed leak check
# --show-leak-kinds=all: Show all types of leaks (definite, indirect, possible, reachable)
# --track-origins=yes: Track where uninitialized values come from
# --keep-debuginfo=yes: Keep debug info for better stack traces
# --num-callers=30: Increase stack trace depth
valgrind --leak-check=full \
         --show-leak-kinds=all \
         --track-origins=yes \
         --keep-debuginfo=yes \
         --num-callers=30 \
         --log-file=$LOG_FILE \
         $BIN_PATH &

SERVER_PID=$!
echo "Server PID: $SERVER_PID"

# Wait for the specified duration
sleep $DURATION

echo "---------------------------------------------------"
echo "Time's up! Stopping server (PID: $SERVER_PID)..."

# Send SIGINT (Ctrl+C) to allow graceful shutdown and Valgrind report generation
kill -SIGINT $SERVER_PID

# Wait for process to exit
wait $SERVER_PID 2>/dev/null

echo "Valgrind analysis complete."
echo "---------------------------------------------------"
echo "Displaying Summary from $LOG_FILE:"
echo ""

# Extract and display the summary section
grep -A 20 "HEAP SUMMARY" $LOG_FILE

echo ""
echo "---------------------------------------------------"
echo "Checking for DEFINITELY LOST blocks:"
grep -A 10 "definitely lost:" $LOG_FILE

echo ""
echo "---------------------------------------------------"
echo "Checking for INDIRECTLY LOST blocks:"
grep -A 10 "indirectly lost:" $LOG_FILE

echo ""
echo "Full report is available in $LOG_FILE"
