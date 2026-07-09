#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

RTSP_ADDR="${RTSP_ADDR:-127.0.0.1:1554}"
CONTROL_ADDR="${CONTROL_ADDR:-127.0.0.1:18892}"
STREAM_APP="${STREAM_APP:-live}"
STREAM_NAME="${STREAM_NAME:-rtsp_transport_smoke}"
INPUT_FILE="${INPUT_FILE:-test_media_files/bbb_sunflower_1080p_30fps_normal.flv}"
RUST_LOG_LEVEL="${RUST_LOG_LEVEL:-warn}"
STARTUP_WAIT_SECS="${STARTUP_WAIT_SECS:-20}"
PUSH_WARMUP_SECS="${PUSH_WARMUP_SECS:-6}"
PROBE_TIMEOUT_SECS="${PROBE_TIMEOUT_SECS:-15}"

RTSP_URL="rtsp://${RTSP_ADDR}/${STREAM_APP}/${STREAM_NAME}"
SERVER_LOG="/tmp/cheetah-rtsp-smoke-server.log"
PUSH_LOG="/tmp/cheetah-rtsp-smoke-push.log"

SERVER_PID=""
PUSH_PID=""
TMP_CONFIG=""

cleanup() {
  if [[ -n "${PUSH_PID}" ]] && kill -0 "${PUSH_PID}" >/dev/null 2>&1; then
    kill "${PUSH_PID}" >/dev/null 2>&1 || true
    wait "${PUSH_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" >/dev/null 2>&1; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${TMP_CONFIG}" ]] && [[ -f "${TMP_CONFIG}" ]]; then
    rm -f "${TMP_CONFIG}"
  fi
}
trap cleanup EXIT

require_cmd() {
  local cmd="$1"
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "[rtsp-smoke] missing required command: ${cmd}" >&2
    exit 1
  fi
}

require_cmd cargo
require_cmd ffmpeg
require_cmd ffprobe
require_cmd python3
require_cmd timeout

if [[ ! -f "${INPUT_FILE}" ]]; then
  echo "[rtsp-smoke] input media file not found: ${INPUT_FILE}" >&2
  exit 1
fi

TMP_CONFIG="$(mktemp /tmp/cheetah-rtsp-smoke.XXXXXX.yaml)"
cat > "${TMP_CONFIG}" <<YAML
global:
  control:
    listen: ${CONTROL_ADDR}
modules:
  rtsp:
    enabled: true
    listen: ${RTSP_ADDR}
    session_timeout_secs: 60
    multicast:
      enabled: true
    pull_jobs: []
    push_jobs: []
    relay_jobs: []
YAML

echo "[rtsp-smoke] starting cheetah-server with rtsp feature"
env -u M7S_GLOBAL__control__listen \
  -u M7S_MODULE__rtsp__enabled \
  -u CHEETAH_CONFIG \
  CHEETAH_CONFIG="${TMP_CONFIG}" \
  RUST_LOG="${RUST_LOG_LEVEL}" \
  cargo run -p cheetah-server --no-default-features --features rtsp > "${SERVER_LOG}" 2>&1 &
SERVER_PID=$!

echo "[rtsp-smoke] waiting for ${RTSP_ADDR} listen"
python3 - "${RTSP_ADDR}" "${STARTUP_WAIT_SECS}" <<'PY'
import socket
import sys
import time

addr = sys.argv[1]
timeout_secs = float(sys.argv[2])
host, port = addr.rsplit(":", 1)
port = int(port)

deadline = time.time() + timeout_secs
last_err = None
while time.time() < deadline:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(1.0)
    try:
        sock.connect((host, port))
        raise SystemExit(0)
    except OSError as exc:
        last_err = exc
        time.sleep(0.2)
    finally:
        sock.close()

raise SystemExit(f"listen check timeout for {addr}: {last_err}")
PY

echo "[rtsp-smoke] publishing RTSP stream ${RTSP_URL}"
ffmpeg -v error -re -stream_loop -1 \
  -i "${INPUT_FILE}" \
  -c copy \
  -f rtsp -rtsp_transport tcp "${RTSP_URL}" > "${PUSH_LOG}" 2>&1 &
PUSH_PID=$!

sleep "${PUSH_WARMUP_SECS}"

probe_transport() {
  local label="$1"
  local rtsp_transport="$2"
  local out_file err_file packet_count

  out_file="/tmp/cheetah-rtsp-smoke-${label}.out"
  err_file="/tmp/cheetah-rtsp-smoke-${label}.err"

  echo "[rtsp-smoke] probing ${label} transport"
  if ! timeout "${PROBE_TIMEOUT_SECS}" \
    ffprobe -v error \
      -rtsp_transport "${rtsp_transport}" \
      -select_streams v:0 \
      -show_packets \
      -show_entries packet=pts_time \
      -of csv=p=0 \
      -read_intervals "%+#5" \
      "${RTSP_URL}" > "${out_file}" 2> "${err_file}"; then
    echo "[rtsp-smoke] ${label} probe command failed" >&2
    cat "${err_file}" >&2 || true
    return 1
  fi

  packet_count="$(grep -c '.' "${out_file}" || true)"
  if [[ "${packet_count}" -lt 1 ]]; then
    echo "[rtsp-smoke] ${label} probe did not receive RTP-backed packets" >&2
    cat "${err_file}" >&2 || true
    return 1
  fi

  echo "[rtsp-smoke] ${label} ok: received ${packet_count} packet timestamps"
}

probe_transport "tcp" "tcp"
probe_transport "udp" "udp"
probe_transport "http_tunnel" "http"

echo "[rtsp-smoke] probing multicast transport with native RTSP + UDP group receive"
python3 - "${RTSP_URL}" "${PROBE_TIMEOUT_SECS}" <<'PY'
import re
import socket
import struct
import sys
import time
from urllib.parse import urlparse

rtsp_url = sys.argv[1]
timeout_secs = float(sys.argv[2])
parsed = urlparse(rtsp_url)
host = parsed.hostname or "127.0.0.1"
port = parsed.port or 554

sock = socket.create_connection((host, port), timeout=timeout_secs)
sock.settimeout(timeout_secs)

cseq = 1

def send_request(method: str, url: str, headers: list[tuple[str, str]], body: bytes = b""):
    global cseq
    lines = [f"{method} {url} RTSP/1.0", f"CSeq: {cseq}"]
    cseq += 1
    for k, v in headers:
        lines.append(f"{k}: {v}")
    if body:
        lines.append(f"Content-Length: {len(body)}")
    request = ("\r\n".join(lines) + "\r\n\r\n").encode("utf-8") + body
    sock.sendall(request)
    return read_response()

def read_response():
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = sock.recv(4096)
        if not chunk:
            raise RuntimeError("rtsp response eof before headers")
        data += chunk
    head, rest = data.split(b"\r\n\r\n", 1)
    lines = head.decode("utf-8", errors="replace").split("\r\n")
    status = lines[0]
    headers = {}
    for line in lines[1:]:
        if ":" in line:
            k, v = line.split(":", 1)
            headers[k.strip().lower()] = v.strip()
    content_len = int(headers.get("content-length", "0") or "0")
    body = rest
    while len(body) < content_len:
        body += sock.recv(4096)
    return status, headers, body[:content_len]

def require_status_ok(status: str, ctx: str):
    if " 200 " not in status:
        raise RuntimeError(f"{ctx} failed: {status}")

status, _, _ = send_request("OPTIONS", rtsp_url, [])
require_status_ok(status, "OPTIONS")

status, headers, sdp_body = send_request("DESCRIBE", rtsp_url, [("Accept", "application/sdp")])
require_status_ok(status, "DESCRIBE")
session_base = headers.get("content-base") or headers.get("content-location") or rtsp_url

sdp_text = sdp_body.decode("utf-8", errors="replace")
track_control = None
in_video = False
for line in sdp_text.splitlines():
    line = line.strip()
    if line.startswith("m="):
        in_video = line.startswith("m=video")
    if in_video and line.startswith("a=control:"):
        track_control = line[len("a=control:"):]
        break
if not track_control:
    raise RuntimeError("DESCRIBE did not provide video track control")

if track_control.startswith("rtsp://"):
    track_url = track_control
else:
    base = session_base
    if not base.endswith("/"):
        base = base + "/"
    track_url = base + track_control

status, headers, _ = send_request(
    "SETUP",
    track_url,
    [("Transport", "RTP/AVP;multicast;port=5000-5001")],
)
require_status_ok(status, "SETUP(multicast)")
session_header = headers.get("session")
if not session_header:
    raise RuntimeError("SETUP(multicast) missing Session header")
session_id = session_header.split(";", 1)[0].strip()

transport = headers.get("transport", "")
destination_match = re.search(r"destination=([^;]+)", transport, re.IGNORECASE)
port_match = re.search(r"(?:port|server_port)=([0-9]+)-([0-9]+)", transport, re.IGNORECASE)
if not destination_match or not port_match:
    raise RuntimeError(f"SETUP(multicast) missing destination/port in Transport: {transport}")
group_ip = destination_match.group(1)
rtp_port = int(port_match.group(1))

status, _, _ = send_request("PLAY", rtsp_url, [("Session", session_id)])
require_status_ok(status, "PLAY(multicast)")

udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
udp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
udp.bind(("", rtp_port))
mreq = struct.pack("=4s4s", socket.inet_aton(group_ip), socket.inet_aton("0.0.0.0"))
udp.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)
udp.settimeout(timeout_secs)

received = 0
deadline = time.time() + timeout_secs
while time.time() < deadline and received < 3:
    try:
        packet, _ = udp.recvfrom(2048)
    except TimeoutError:
        continue
    if len(packet) >= 12 and (packet[0] >> 6) == 2:
        received += 1

if received < 1:
    raise RuntimeError(f"multicast probe received no RTP packet from {group_ip}:{rtp_port}")
print(f"multicast ok: received {received} RTP packets from {group_ip}:{rtp_port}")

send_request("TEARDOWN", rtsp_url, [("Session", session_id)])
udp.close()
sock.close()
PY

echo "[rtsp-smoke] all transport probes passed"
