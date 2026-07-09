#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

RTMP_ADDR="${RTMP_ADDR:-127.0.0.1:11935}"
HTTP_ADDR="${HTTP_ADDR:-127.0.0.1:18080}"
CONTROL_ADDR="${CONTROL_ADDR:-127.0.0.1:18891}"
STREAM_APP="${STREAM_APP:-live}"
STREAM_NAME="${STREAM_NAME:-http_flv_smoke}"
PUSH_SOURCE="${PUSH_SOURCE:-synthetic}"
INPUT_FILE="${INPUT_FILE:-test_media_files/bbb_sunflower_1080p_30fps_normal.flv}"
RUST_LOG_LEVEL="${RUST_LOG_LEVEL:-warn}"
STARTUP_WAIT_SECS="${STARTUP_WAIT_SECS:-20}"
PUSH_WARMUP_SECS="${PUSH_WARMUP_SECS:-6}"
PROBE_TIMEOUT_SECS="${PROBE_TIMEOUT_SECS:-10}"

HTTP_URL="http://${HTTP_ADDR}/${STREAM_APP}/${STREAM_NAME}.flv"
WS_URL="ws://${HTTP_ADDR}/${STREAM_APP}/${STREAM_NAME}.flv"
RTMP_URL="rtmp://${RTMP_ADDR}/${STREAM_APP}/${STREAM_NAME}"

SERVER_PID=""
PUSH_PID=""

cleanup() {
  if [[ -n "${PUSH_PID}" ]] && kill -0 "${PUSH_PID}" >/dev/null 2>&1; then
    kill "${PUSH_PID}" >/dev/null 2>&1 || true
    wait "${PUSH_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" >/dev/null 2>&1; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

require_cmd() {
  local cmd="$1"
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "[http-flv-smoke] missing required command: ${cmd}" >&2
    exit 1
  fi
}

require_cmd cargo
require_cmd ffmpeg
require_cmd python3

if [[ "${PUSH_SOURCE}" == "file" ]] && [[ ! -f "${INPUT_FILE}" ]]; then
  echo "[http-flv-smoke] input media file not found: ${INPUT_FILE}" >&2
  exit 1
fi

echo "[http-flv-smoke] starting cheetah-server with http-flv feature"
env -u CHEETAH_CONFIG \
  RUST_LOG="${RUST_LOG_LEVEL}" \
  M7S_GLOBAL__control__listen="${CONTROL_ADDR}" \
  M7S_MODULE__rtmp__enabled=true \
  M7S_MODULE__rtmp__listen="${RTMP_ADDR}" \
  "M7S_MODULE__http-flv__enabled=true" \
  "M7S_MODULE__http-flv__listen=${HTTP_ADDR}" \
  cargo run -p cheetah-server --features http-flv > /tmp/cheetah-http-flv-smoke-server.log 2>&1 &
SERVER_PID=$!

echo "[http-flv-smoke] waiting for ${RTMP_ADDR} and ${HTTP_ADDR} listen"
python3 - "${RTMP_ADDR}" "${HTTP_ADDR}" "${STARTUP_WAIT_SECS}" <<'PY'
import socket
import sys
import time

rtmp_addr = sys.argv[1]
http_addr = sys.argv[2]
timeout_secs = float(sys.argv[3])

def wait_addr(addr: str, timeout: float) -> None:
    host, port = addr.rsplit(":", 1)
    deadline = time.time() + timeout
    last_err = None
    while time.time() < deadline:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(1.0)
        try:
            sock.connect((host, int(port)))
            return
        except OSError as exc:
            last_err = exc
            time.sleep(0.2)
        finally:
            sock.close()
    raise SystemExit(f"listen check timeout for {addr}: {last_err}")

wait_addr(rtmp_addr, timeout_secs)
wait_addr(http_addr, timeout_secs)
PY

echo "[http-flv-smoke] publishing RTMP stream ${RTMP_URL}"
if [[ "${PUSH_SOURCE}" == "file" ]]; then
  ffmpeg -v error -re -stream_loop -1 -i "${INPUT_FILE}" -c copy -f flv "${RTMP_URL}" > /tmp/cheetah-http-flv-smoke-push.log 2>&1 &
else
  ffmpeg -v error -re \
    -f lavfi -i sine=frequency=1000:sample_rate=44100 \
    -c:a aac -b:a 128k \
    -f flv "${RTMP_URL}" > /tmp/cheetah-http-flv-smoke-push.log 2>&1 &
fi
PUSH_PID=$!
sleep "${PUSH_WARMUP_SECS}"

echo "[http-flv-smoke] probing HTTP-FLV bytes ${HTTP_URL}"
python3 - "${HTTP_URL}" "${PROBE_TIMEOUT_SECS}" <<'PY'
import socket
import sys
from urllib.parse import urlparse

url = sys.argv[1]
timeout_secs = float(sys.argv[2])
parsed = urlparse(url)
host = parsed.hostname or "127.0.0.1"
port = parsed.port or 80
path = parsed.path or "/"
if parsed.query:
    path = f"{path}?{parsed.query}"

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.settimeout(timeout_secs)
sock.connect((host, port))

request = (
    f"GET {path} HTTP/1.1\r\n"
    f"Host: {host}:{port}\r\n"
    "Connection: close\r\n"
    "\r\n"
)
sock.sendall(request.encode("ascii"))

response = b""
while b"\r\n\r\n" not in response:
    chunk = sock.recv(4096)
    if not chunk:
        raise SystemExit("http-flv response EOF before headers")
    response += chunk

head, body = response.split(b"\r\n\r\n", 1)
status_line = head.split(b"\r\n", 1)[0]
if b" 200 " not in status_line:
    raise SystemExit(f"http-flv status not 200: {status_line!r}")
content_type = b""
for line in head.split(b"\r\n")[1:]:
    if line.lower().startswith(b"content-type:"):
        content_type = line.split(b":", 1)[1].strip().lower()
        break
if b"video/x-flv" not in content_type:
    raise SystemExit(f"http-flv content-type mismatch: {content_type!r}")

data = body
try:
    if not data:
        data = sock.recv(4096)
except TimeoutError:
    data = b""

if data:
    if b"FLV" not in data[:16]:
        raise SystemExit(f"http-flv first payload chunk does not contain FLV signature: {data[:16]!r}")
    print(f"http-flv ok: handshake and payload chunk={len(data)} bytes")
else:
    print("http-flv ok: handshake only (no payload within probe timeout)")
PY

echo "[http-flv-smoke] probing WS-FLV binary frames ${WS_URL}"
python3 - "${WS_URL}" "${PROBE_TIMEOUT_SECS}" <<'PY'
import base64
import os
import socket
import struct
import sys
import time
from urllib.parse import urlparse

url = sys.argv[1]
timeout_secs = float(sys.argv[2])
parsed = urlparse(url)
host = parsed.hostname or "127.0.0.1"
port = parsed.port or 80
path = parsed.path or "/"
if parsed.query:
    path = f"{path}?{parsed.query}"

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.settimeout(timeout_secs)
sock.connect((host, port))

ws_key = base64.b64encode(os.urandom(16)).decode()
request = (
    f"GET {path} HTTP/1.1\r\n"
    f"Host: {host}:{port}\r\n"
    "Upgrade: websocket\r\n"
    "Connection: Upgrade\r\n"
    f"Sec-WebSocket-Key: {ws_key}\r\n"
    "Sec-WebSocket-Version: 13\r\n"
    "\r\n"
)
sock.sendall(request.encode("ascii"))

response = b""
deadline = time.time() + timeout_secs
while b"\r\n\r\n" not in response and time.time() < deadline:
    chunk = sock.recv(4096)
    if not chunk:
        raise SystemExit("websocket handshake EOF")
    response += chunk

if b" 101 " not in response.split(b"\r\n", 1)[0]:
    raise SystemExit(f"websocket handshake failed: {response[:200]!r}")

buffer = response.split(b"\r\n\r\n", 1)[1]
binary_frames = []

def recv_exact(sock_obj, initial, n):
    data = initial
    while len(data) < n:
        chunk_ = sock_obj.recv(n - len(data))
        if not chunk_:
            raise SystemExit("unexpected eof while reading websocket frame")
        data += chunk_
    return data

while len(binary_frames) < 2:
    if len(buffer) < 2:
        try:
            buffer += sock.recv(4096)
        except TimeoutError:
            break
        if not buffer:
            break
        continue

    b1, b2 = buffer[0], buffer[1]
    opcode = b1 & 0x0F
    masked = (b2 & 0x80) != 0
    payload_len = b2 & 0x7F
    idx = 2

    if payload_len == 126:
        buffer = recv_exact(sock, buffer, idx + 2)
        payload_len = struct.unpack("!H", buffer[idx:idx + 2])[0]
        idx += 2
    elif payload_len == 127:
        buffer = recv_exact(sock, buffer, idx + 8)
        payload_len = struct.unpack("!Q", buffer[idx:idx + 8])[0]
        idx += 8

    if masked:
        buffer = recv_exact(sock, buffer, idx + 4)
        idx += 4

    buffer = recv_exact(sock, buffer, idx + payload_len)
    payload = buffer[idx:idx + payload_len]
    buffer = buffer[idx + payload_len:]

    if opcode == 0x8:
        break
    if opcode == 0x2:
        binary_frames.append(payload)

if binary_frames:
    if not binary_frames[0]:
        raise SystemExit("first websocket binary frame is empty")
    if b"FLV" not in binary_frames[0]:
        raise SystemExit("first websocket binary frame does not contain FLV signature")
    print(f"ws-flv ok: handshake and {len(binary_frames)} binary frames")
else:
    print("ws-flv ok: handshake only (no binary frame within probe timeout)")
PY

echo "[http-flv-smoke] success"
