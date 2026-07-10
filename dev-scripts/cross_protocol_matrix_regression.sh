#!/usr/bin/env bash
set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

MATRIX_SCRIPT="${MATRIX_SCRIPT:-${ROOT_DIR}/dev-scripts/cross_protocol_matrix_command_templates.sh}"
MATRIX_ACCEPTANCE_FILE="${MATRIX_ACCEPTANCE_FILE:-${ROOT_DIR}/dev-scripts/cross_protocol_matrix_acceptance_matrix.tsv}"
INPUT_PROFILE="${INPUT_PROFILE:-b-frame-h264}"
MATRIX_PROFILE_MODE="${MATRIX_PROFILE_MODE:-all}"
REPORT_ROOT="${REPORT_ROOT:-${ROOT_DIR}/dev-scripts/reports/cross-protocol-matrix}"
SCENARIO_DURATION_SECONDS="${SCENARIO_DURATION_SECONDS:-300}"
PUSH_STARTUP_GRACE_MS="${PUSH_STARTUP_GRACE_MS:-800}"
STARTUP_POLL_INTERVAL_MS="${STARTUP_POLL_INTERVAL_MS:-50}"
FREEZE_LOG_REGEX="${FREEZE_LOG_REGEX:-freeze|stutter}"
DTS_OUT_OF_ORDER_REGEX="${DTS_OUT_OF_ORDER_REGEX:-dts out of order}"
INVALID_TIMESTAMPS_REGEX="${INVALID_TIMESTAMPS_REGEX:-invalid timestamps}"
NON_INCREASING_DTS_REGEX="${NON_INCREASING_DTS_REGEX:-non-increasing dts}"
NEGATIVE_CTS_REGEX="${NEGATIVE_CTS_REGEX:-negative cts}"
ENABLE_FFPROBE_CHECKS="${ENABLE_FFPROBE_CHECKS:-1}"
FFPROBE_BIN="${FFPROBE_BIN:-ffprobe}"
FFPROBE_TIMEOUT_SECONDS="${FFPROBE_TIMEOUT_SECONDS:-8}"
FFPROBE_START_PTS_NEAR_ZERO_MS="${FFPROBE_START_PTS_NEAR_ZERO_MS:-1000}"
ENABLE_TCPDUMP_CAPTURE="${ENABLE_TCPDUMP_CAPTURE:-0}"
TCPDUMP_BIN="${TCPDUMP_BIN:-tcpdump}"
TCPDUMP_INTERFACE="${TCPDUMP_INTERFACE:-any}"
RTSP_PORT="${RTSP_PORT:-8554}"
RTMP_PORT="${RTMP_PORT:-1935}"
SOURCE_REPAIR_REGEX="${SOURCE_REPAIR_REGEX:-source_disorder|source timeline repair|raw_timestamp_ms}"
CANONICAL_REPAIR_REGEX="${CANONICAL_REPAIR_REGEX:-canonical_repair|canonical timeline repaired}"
EGRESS_REPAIR_REGEX="${EGRESS_REPAIR_REGEX:-egress repair|monotonic egress}"
REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD="${REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD:-32}"

CURRENT_PUSH_PID=""
CURRENT_PULL_PID=""
CURRENT_FFPROBE_PID=""
CURRENT_TCPDUMP_PID=""

log() {
  echo "[cross-protocol-regression] $*"
}

fail() {
  log "error: $*" >&2
  exit 1
}

usage() {
  cat <<USAGE
Usage:
  ${SCRIPT_NAME} list
  ${SCRIPT_NAME} run <scenario>
  ${SCRIPT_NAME} run-all
  ${SCRIPT_NAME} doctor

Environment overrides:
  MATRIX_SCRIPT, MATRIX_ACCEPTANCE_FILE, INPUT_PROFILE, MATRIX_PROFILE_MODE, REPORT_ROOT,
  SCENARIO_DURATION_SECONDS, PUSH_STARTUP_GRACE_MS,
  STARTUP_POLL_INTERVAL_MS, FREEZE_LOG_REGEX,
  DTS_OUT_OF_ORDER_REGEX, INVALID_TIMESTAMPS_REGEX,
  NON_INCREASING_DTS_REGEX, NEGATIVE_CTS_REGEX,
  ENABLE_FFPROBE_CHECKS, FFPROBE_BIN, FFPROBE_TIMEOUT_SECONDS,
  FFPROBE_START_PTS_NEAR_ZERO_MS,
  SOURCE_REPAIR_REGEX, CANONICAL_REPAIR_REGEX, EGRESS_REPAIR_REGEX,
  REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD,
  ENABLE_TCPDUMP_CAPTURE, TCPDUMP_BIN, TCPDUMP_INTERFACE,
  RTSP_PORT, RTMP_PORT
USAGE
}

now_ms() {
  date +%s%3N
}

sleep_ms() {
  local value_ms="$1"
  awk -v v="${value_ms}" 'BEGIN { printf "%.3f", v / 1000.0 }'
}

cleanup_processes() {
  local pid
  for pid in "${CURRENT_PULL_PID}" "${CURRENT_PUSH_PID}"; do
    [[ -n "${pid}" ]] || continue
    if kill -0 "${pid}" >/dev/null 2>&1; then
      kill -TERM "${pid}" >/dev/null 2>&1 || true
      sleep 0.1
      kill -KILL "${pid}" >/dev/null 2>&1 || true
    fi
  done
  if [[ -n "${CURRENT_FFPROBE_PID}" ]] && kill -0 "${CURRENT_FFPROBE_PID}" >/dev/null 2>&1; then
    kill -TERM "${CURRENT_FFPROBE_PID}" >/dev/null 2>&1 || true
    sleep 0.1
    kill -KILL "${CURRENT_FFPROBE_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${CURRENT_TCPDUMP_PID}" ]] && kill -0 "${CURRENT_TCPDUMP_PID}" >/dev/null 2>&1; then
    kill -TERM "${CURRENT_TCPDUMP_PID}" >/dev/null 2>&1 || true
    sleep 0.1
    kill -KILL "${CURRENT_TCPDUMP_PID}" >/dev/null 2>&1 || true
  fi
}

trap cleanup_processes EXIT

require_tools() {
  command -v awk >/dev/null 2>&1 || fail "missing binary: awk"
  command -v grep >/dev/null 2>&1 || fail "missing binary: grep"
  command -v timeout >/dev/null 2>&1 || fail "missing binary: timeout"
  command -v date >/dev/null 2>&1 || fail "missing binary: date"
  if [[ "${ENABLE_FFPROBE_CHECKS}" == "1" ]]; then
    command -v "${FFPROBE_BIN}" >/dev/null 2>&1 || fail "missing binary: ${FFPROBE_BIN}"
  fi
  if [[ "${ENABLE_TCPDUMP_CAPTURE}" == "1" ]]; then
    command -v "${TCPDUMP_BIN}" >/dev/null 2>&1 || fail "missing binary: ${TCPDUMP_BIN}"
  fi
}

ensure_matrix_script() {
  [[ -x "${MATRIX_SCRIPT}" ]] || fail "matrix script is not executable: ${MATRIX_SCRIPT}"
}

iter_acceptance_rows() {
  [[ -f "${MATRIX_ACCEPTANCE_FILE}" ]] || fail "matrix acceptance file not found: ${MATRIX_ACCEPTANCE_FILE}"

  while IFS= read -r line || [[ -n "${line}" ]]; do
    [[ -n "${line}" ]] || continue
    [[ "${line}" == \#* ]] && continue
    echo "${line}"
  done < "${MATRIX_ACCEPTANCE_FILE}"
}

acceptance_operator() {
  local key="$1"
  case "${key}" in
    startup_latency)
      echo "<="
      ;;
    first_keyframe_delay_ms)
      echo "<="
      ;;
    continuous_play)
      echo ">="
      ;;
    freeze_events|dts_out_of_order)
      echo "=="
      ;;
    invalid_timestamps|non_increasing_dts|negative_cts)
      echo "=="
      ;;
    ffprobe_first_video_keyframe|ffprobe_first_video_pts_near_zero|ffprobe_video_dts_monotonic)
      echo "=="
      ;;
    source_repair_events|canonical_repair_events|egress_repair_events)
      echo "=="
      ;;
    repair_warn_high_frequency|repair_context_complete)
      echo "=="
      ;;
    *)
      fail "unsupported acceptance check key '${key}'"
      ;;
  esac
}

is_required_check() {
  local required="$1"
  case "${required}" in
    yes)
      return 0
      ;;
    no)
      return 1
      ;;
    *)
      fail "invalid required flag '${required}', expected 'yes' or 'no'"
      ;;
  esac
}

compare_metric() {
  local key="$1"
  local actual="$2"
  local threshold="$3"
  local operator
  local effective_threshold="${threshold}"

  # For short exploratory runs, the effective playback target cannot exceed the
  # configured scenario duration. This keeps the acceptance matrix unchanged for
  # full 300s runs while allowing shorter runs to validate continuous playback.
  if [[ "${key}" == "continuous_play" && -n "${SCENARIO_DURATION_SECONDS:-}" && "${threshold}" -gt "${SCENARIO_DURATION_SECONDS}" ]]; then
    effective_threshold="${SCENARIO_DURATION_SECONDS}"
  fi

  operator="$(acceptance_operator "${key}")"
  if (( actual < 0 )); then
    case "${key}" in
      startup_latency|first_keyframe_delay_ms|continuous_play|ffprobe_first_video_keyframe|ffprobe_first_video_pts_near_zero|ffprobe_video_dts_monotonic|repair_warn_high_frequency|repair_context_complete)
        return 1
        ;;
    esac
  fi

  case "${operator}" in
    "<=")
      (( actual <= effective_threshold ))
      ;;
    ">=")
      (( actual >= effective_threshold ))
      ;;
    "==")
      (( actual == effective_threshold ))
      ;;
    *)
      fail "unsupported acceptance operator '${operator}'"
      ;;
  esac
}

scenario_commands() {
  local scenario="$1"
  local profile="$2"
  local rendered
  local line
  local push_cmd=""
  local pull_cmd=""

  rendered="$(INPUT_PROFILE="${profile}" "${MATRIX_SCRIPT}" show "${scenario}")" \
    || fail "failed to render scenario '${scenario}' with profile '${profile}'"

  while IFS= read -r line || [[ -n "${line}" ]]; do
    [[ -n "${line}" ]] || continue
    [[ "${line}" == \#* ]] && continue

    if [[ -z "${push_cmd}" ]]; then
      push_cmd="${line}"
      continue
    fi

    if [[ -z "${pull_cmd}" ]]; then
      pull_cmd="${line}"
      break
    fi
  done <<< "${rendered}"

  [[ -n "${push_cmd}" ]] || fail "failed to parse push command for scenario '${scenario}'"
  [[ -n "${pull_cmd}" ]] || fail "failed to parse pull command for scenario '${scenario}'"

  printf '%s\n%s\n' "${push_cmd}" "${pull_cmd}"
}

doctor() {
  require_tools
  ensure_matrix_script
  "${MATRIX_SCRIPT}" doctor >/dev/null

  local row_count=0
  local row
  while IFS= read -r row; do
    IFS='|' read -r check_key required threshold unit description <<< "${row}"
    [[ -n "${check_key}" && -n "${required}" && -n "${threshold}" && -n "${unit}" && -n "${description}" ]] || fail "invalid acceptance row: ${row}"
    [[ "${threshold}" =~ ^[0-9]+$ ]] || fail "invalid threshold '${threshold}' for check '${check_key}'"
    is_required_check "${required}" || true
    acceptance_operator "${check_key}" >/dev/null
    row_count=$((row_count + 1))
  done < <(iter_acceptance_rows)

  (( row_count > 0 )) || fail "matrix acceptance file has no data rows: ${MATRIX_ACCEPTANCE_FILE}"

  log "matrix script: ${MATRIX_SCRIPT}"
  log "matrix acceptance file: ${MATRIX_ACCEPTANCE_FILE}"
  log "input profile mode: ${MATRIX_PROFILE_MODE}"
  log "selected input profile: ${INPUT_PROFILE}"
  log "ffprobe checks enabled: ${ENABLE_FFPROBE_CHECKS}"
  if [[ "${ENABLE_FFPROBE_CHECKS}" == "1" ]]; then
    log "ffprobe binary: ${FFPROBE_BIN}"
    log "ffprobe timeout: ${FFPROBE_TIMEOUT_SECONDS}s"
    log "ffprobe first pts near zero threshold: ${FFPROBE_START_PTS_NEAR_ZERO_MS}ms"
  fi
  log "source repair regex: ${SOURCE_REPAIR_REGEX}"
  log "canonical repair regex: ${CANONICAL_REPAIR_REGEX}"
  log "egress repair regex: ${EGRESS_REPAIR_REGEX}"
  log "repair high-frequency threshold: ${REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD}"
  log "tcpdump capture enabled: ${ENABLE_TCPDUMP_CAPTURE}"
  if [[ "${ENABLE_TCPDUMP_CAPTURE}" == "1" ]]; then
    log "tcpdump binary: ${TCPDUMP_BIN}"
    log "tcpdump interface: ${TCPDUMP_INTERFACE}"
    log "tcpdump ports filter: rtsp=${RTSP_PORT}, rtmp=${RTMP_PORT}"
  fi
  log "report root: ${REPORT_ROOT}"
  log "scenario duration: ${SCENARIO_DURATION_SECONDS}s"
  log "doctor check passed"
}

metric_value() {
  local key="$1"
  local startup_latency_ms="$2"
  local playback_seconds="$3"
  local freeze_events="$4"
  local dts_out_of_order="$5"
  local invalid_timestamps="$6"
  local non_increasing_dts="$7"
  local negative_cts="$8"
  local ffprobe_first_video_keyframe="$9"
  local ffprobe_first_video_pts_near_zero="${10}"
  local ffprobe_video_dts_monotonic="${11}"
  local first_keyframe_delay_ms="${12}"
  local source_repair_events="${13}"
  local canonical_repair_events="${14}"
  local egress_repair_events="${15}"
  local repair_warn_high_frequency="${16}"
  local repair_context_complete="${17}"

  case "${key}" in
    startup_latency)
      echo "${startup_latency_ms}"
      ;;
    first_keyframe_delay_ms)
      echo "${first_keyframe_delay_ms}"
      ;;
    continuous_play)
      echo "${playback_seconds}"
      ;;
    freeze_events)
      echo "${freeze_events}"
      ;;
    dts_out_of_order)
      echo "${dts_out_of_order}"
      ;;
    invalid_timestamps)
      echo "${invalid_timestamps}"
      ;;
    non_increasing_dts)
      echo "${non_increasing_dts}"
      ;;
    negative_cts)
      echo "${negative_cts}"
      ;;
    ffprobe_first_video_keyframe)
      echo "${ffprobe_first_video_keyframe}"
      ;;
    ffprobe_first_video_pts_near_zero)
      echo "${ffprobe_first_video_pts_near_zero}"
      ;;
    ffprobe_video_dts_monotonic)
      echo "${ffprobe_video_dts_monotonic}"
      ;;
    source_repair_events)
      echo "${source_repair_events}"
      ;;
    canonical_repair_events)
      echo "${canonical_repair_events}"
      ;;
    egress_repair_events)
      echo "${egress_repair_events}"
      ;;
    repair_warn_high_frequency)
      echo "${repair_warn_high_frequency}"
      ;;
    repair_context_complete)
      echo "${repair_context_complete}"
      ;;
    *)
      fail "unsupported acceptance check key '${key}'"
      ;;
  esac
}

count_pattern_matches() {
  local pattern="$1"
  local file="$2"
  grep -Eic "${pattern}" "${file}" || true
}

extract_first_second_avg_interval_ms() {
  local pull_log="$1"
  awk '
    {
      line=$0
      while (match(line, /(pts_time|pkt_pts_time):(-?[0-9]+(\.[0-9]+)?)/)) {
        token=substr(line, RSTART, RLENGTH)
        sub(/^[^:]*:/, "", token)
        values[++n]=token + 0.0
        line=substr(line, RSTART + RLENGTH)
      }
    }
    END {
      if (n < 2) {
        print "-1"
        exit
      }
      first=values[1]
      interval_count=0
      interval_sum=0.0
      for (i=2; i<=n; i++) {
        delta=values[i] - values[i-1]
        elapsed=values[i] - first
        if (elapsed > 1.0) {
          break
        }
        if (delta < 0) {
          continue
        }
        interval_sum += delta
        interval_count++
      }
      if (interval_count == 0) {
        print "-1"
        exit
      }
      print int((interval_sum / interval_count) * 1000 + 0.5)
    }
  ' "${pull_log}"
}

extract_media_time_span_seconds() {
  local pull_log="$1"
  awk '
    {
      line=$0
      while (match(line, /(pts_time|pkt_pts_time):(-?[0-9]+(\.[0-9]+)?)/)) {
        token=substr(line, RSTART, RLENGTH)
        sub(/^[^:]*:/, "", token)
        value=token + 0.0
        if (!has_first) {
          first=value
          has_first=1
        }
        last=value
        line=substr(line, RSTART + RLENGTH)
      }
    }
    END {
      if (!has_first) {
        print "-1"
        exit
      }
      span=last-first
      if (span < 0) {
        span=0
      }
      printf "%.6f\n", span
    }
  ' "${pull_log}"
}

estimate_average_playback_rate_x() {
  local media_span_seconds="$1"
  local playback_duration_ms="$2"
  awk -v media_span="${media_span_seconds}" -v wall_ms="${playback_duration_ms}" '
    BEGIN {
      if (media_span < 0 || wall_ms <= 0) {
        print "-1"
        exit
      }
      wall_seconds=wall_ms / 1000.0
      if (wall_seconds <= 0) {
        print "-1"
        exit
      }
      printf "%.3f\n", media_span / wall_seconds
    }
  '
}

list_profiles() {
  case "${MATRIX_PROFILE_MODE}" in
    all)
      local listed
      if ! listed="$("${MATRIX_SCRIPT}" list-inputs 2>/dev/null)"; then
        echo "${INPUT_PROFILE}"
        return 0
      fi

      local found=0
      local line
      while IFS= read -r line || [[ -n "${line}" ]]; do
        [[ -n "${line}" ]] || continue
        local profile
        profile="${line%%|*}"
        [[ -n "${profile}" ]] || continue
        found=1
        echo "${profile}"
      done <<< "${listed}"

      if (( found == 0 )); then
        echo "${INPUT_PROFILE}"
      fi
      ;;
    selected)
      echo "${INPUT_PROFILE}"
      ;;
    *)
      fail "invalid MATRIX_PROFILE_MODE '${MATRIX_PROFILE_MODE}', expected 'all' or 'selected'"
      ;;
  esac
}

pull_target_from_command() {
  local pull_cmd="$1"
  # Pull commands now use '-i <url> ... [sink]' so extract the URL after '-i'.
  awk '{
    for (i=1; i<NF; i++) {
      if ($i == "-i") { print $(i+1); exit }
    }
    print $NF
  }' <<< "${pull_cmd}"
}

pull_rtsp_transport_from_command() {
  local pull_cmd="$1"
  if [[ "${pull_cmd}" =~ -rtsp_transport[[:space:]]+([a-zA-Z0-9_]+) ]]; then
    echo "${BASH_REMATCH[1]}"
  else
    echo ""
  fi
}

build_ffprobe_command_for_pull() {
  local pull_cmd="$1"
  local target_url
  target_url="$(pull_target_from_command "${pull_cmd}")"
  [[ -n "${target_url}" ]] || return 1

  local rtsp_transport
  rtsp_transport="$(pull_rtsp_transport_from_command "${pull_cmd}")"
  if [[ "${target_url}" == rtsp://* ]]; then
    # For RTSP/RTP H.264, a small probe keeps ffprobe from applying SPS-based
    # num_reorder_frames inference to the decode timestamps; the wire RTP
    # timestamps are already monotonic.
    if [[ -n "${rtsp_transport}" ]]; then
      echo "${FFPROBE_BIN} -v error -probesize 32 -analyzeduration 0 -rtsp_transport ${rtsp_transport} -select_streams v:0 -show_entries packet=pts_time,dts_time,flags -of csv=p=0 -read_intervals %+4 \"${target_url}\""
    else
      echo "${FFPROBE_BIN} -v error -probesize 32 -analyzeduration 0 -select_streams v:0 -show_entries packet=pts_time,dts_time,flags -of csv=p=0 -read_intervals %+4 \"${target_url}\""
    fi
  else
    echo "${FFPROBE_BIN} -v error -select_streams v:0 -show_entries packet=pts_time,dts_time,flags -of csv=p=0 -read_intervals %+4 \"${target_url}\""
  fi
}

ffprobe_first_video_keyframe_metric() {
  local ffprobe_log="$1"
  awk -F',' '
    NF >= 3 {
      flags=$3
      gsub(/[[:space:]]/, "", flags)
      if (flags ~ /K/) {
        print 1
      } else {
        print 0
      }
      exit
    }
    END {
      if (NR == 0) {
        print 0
      }
    }
  ' "${ffprobe_log}"
}

ffprobe_first_video_pts_near_zero_metric() {
  local ffprobe_log="$1"
  local threshold_ms="$2"
  awk -F',' -v threshold_ms="${threshold_ms}" '
    NF >= 1 {
      pts=$1 + 0.0
      pts_ms=pts * 1000.0
      if (pts_ms < 0) {
        pts_ms = -pts_ms
      }
      if (pts_ms <= threshold_ms) {
        print 1
      } else {
        print 0
      }
      exit
    }
    END {
      if (NR == 0) {
        print 0
      }
    }
  ' "${ffprobe_log}"
}

ffprobe_video_dts_monotonic_metric() {
  local ffprobe_log="$1"
  awk -F',' '
    BEGIN {
      has = 0
      monotonic = 1
    }
    NF >= 2 {
      dts=$2 + 0.0
      if (!has) {
        last=dts
        has=1
        next
      }
      if (dts < last) {
        monotonic=0
      }
      last=dts
    }
    END {
      if (!has) {
        print 0
      } else if (monotonic == 1) {
        print 1
      } else {
        print 0
      }
    }
  ' "${ffprobe_log}"
}

ffprobe_first_keyframe_delay_ms_metric() {
  local ffprobe_log="$1"
  awk -F',' '
    BEGIN {
      has_first = 0
      has_key = 0
    }
    NF >= 3 {
      pts=$1 + 0.0
      flags=$3
      gsub(/[[:space:]]/, "", flags)
      if (!has_first) {
        first_pts=pts
        has_first=1
      }
      if (!has_key && flags ~ /K/) {
        key_pts=pts
        has_key=1
      }
    }
    END {
      if (!has_first || !has_key) {
        print "-1"
        exit
      }
      delay_ms=(key_pts-first_pts) * 1000.0
      if (delay_ms < 0) {
        delay_ms = -delay_ms
      }
      print int(delay_ms + 0.5)
    }
  ' "${ffprobe_log}"
}

repair_context_complete_metric() {
  local push_log="$1"
  local pull_log="$2"
  local combined_repair_regex="${SOURCE_REPAIR_REGEX}|${CANONICAL_REPAIR_REGEX}|${EGRESS_REPAIR_REGEX}"
  awk -v repair_regex="${combined_repair_regex}" '
    BEGIN {
      total = 0
      complete = 0
    }
    {
      lower=tolower($0)
      if (lower ~ repair_regex) {
        total++
        has_source=(lower ~ /source[_ -]?(pts|dts|timestamp)|raw_timestamp_ms|rtp_timestamp|rtmp_timestamp/)
        has_canonical=(lower ~ /(^|[^a-z])(pts|dts)=|canonical[_ -]?(pts|dts)|pts_ms=|dts_ms=/)
        if (has_source && has_canonical) {
          complete++
        }
      }
    }
    END {
      if (total == 0) {
        print 1
      } else if (complete == total) {
        print 1
      } else {
        print 0
      }
    }
  ' "${push_log}" "${pull_log}"
}

run_scenario() {
  local scenario="$1"
  local profile="$2"
  local run_id="$3"
  local scenario_dir="${REPORT_ROOT}/${run_id}/${profile}/${scenario}"
  local pull_log="${scenario_dir}/pull.log"
  local push_log="${scenario_dir}/push.log"
  local ffprobe_log="${scenario_dir}/ffprobe.log"
  local pcap_file="${scenario_dir}/capture.pcap"
  local summary_file="${scenario_dir}/summary.txt"
  local anomaly_file="${scenario_dir}/anomaly_summary.txt"
  local failure_input_file="${scenario_dir}/failure_input.txt"

  mkdir -p "${scenario_dir}"

  local commands
  local push_cmd
  local pull_cmd
  commands="$(scenario_commands "${scenario}" "${profile}")"
  push_cmd="$(printf '%s\n' "${commands}" | sed -n '1p')"
  pull_cmd="$(printf '%s\n' "${commands}" | sed -n '2p')"

  log "scenario '${scenario}' started (profile='${profile}')"
  log "push command: ${push_cmd}"
  log "pull command: ${pull_cmd}"
  if [[ "${ENABLE_TCPDUMP_CAPTURE}" == "1" ]]; then
    local tcpdump_filter="port ${RTSP_PORT} or port ${RTMP_PORT}"
    timeout --signal=INT --kill-after=2 "$((SCENARIO_DURATION_SECONDS + 15))s" \
      "${TCPDUMP_BIN}" -i "${TCPDUMP_INTERFACE}" -s 0 -w "${pcap_file}" "${tcpdump_filter}" \
      >/dev/null 2>&1 &
    CURRENT_TCPDUMP_PID="$!"
  fi

  bash -lc "${push_cmd}" >"${push_log}" 2>&1 &
  CURRENT_PUSH_PID="$!"
  sleep "$(sleep_ms "${PUSH_STARTUP_GRACE_MS}")"

  local pull_start_ms
  local pull_end_ms
  local startup_latency_ms=-1
  local pull_rc
  local ffprobe_cmd=""
  local ffprobe_rc=0

  timeout --signal=INT --kill-after=5 "${SCENARIO_DURATION_SECONDS}s" bash -lc "${pull_cmd}" >"${pull_log}" 2>&1 &
  CURRENT_PULL_PID="$!"
  pull_start_ms="$(now_ms)"
  if [[ "${ENABLE_FFPROBE_CHECKS}" == "1" ]]; then
    if ffprobe_cmd="$(build_ffprobe_command_for_pull "${pull_cmd}")"; then
      timeout --signal=INT --kill-after=2 "${FFPROBE_TIMEOUT_SECONDS}s" bash -lc "${ffprobe_cmd}" >"${ffprobe_log}" 2>&1 &
      CURRENT_FFPROBE_PID="$!"
    fi
  fi

  while kill -0 "${CURRENT_PULL_PID}" >/dev/null 2>&1; do
    if [[ ${startup_latency_ms} -lt 0 && -s "${pull_log}" ]]; then
      startup_latency_ms=$(( $(now_ms) - pull_start_ms ))
    fi
    sleep "$(sleep_ms "${STARTUP_POLL_INTERVAL_MS}")"
  done

  set +e
  wait "${CURRENT_PULL_PID}"
  pull_rc=$?
  set -e
  CURRENT_PULL_PID=""
  pull_end_ms="$(now_ms)"

  if [[ -n "${CURRENT_FFPROBE_PID}" ]]; then
    set +e
    wait "${CURRENT_FFPROBE_PID}"
    ffprobe_rc=$?
    set -e
    CURRENT_FFPROBE_PID=""
  fi

  if kill -0 "${CURRENT_PUSH_PID}" >/dev/null 2>&1; then
    kill -TERM "${CURRENT_PUSH_PID}" >/dev/null 2>&1 || true
  fi
  set +e
  wait "${CURRENT_PUSH_PID}" >/dev/null 2>&1
  set -e
  CURRENT_PUSH_PID=""
  if [[ -n "${CURRENT_TCPDUMP_PID}" ]]; then
    if kill -0 "${CURRENT_TCPDUMP_PID}" >/dev/null 2>&1; then
      kill -TERM "${CURRENT_TCPDUMP_PID}" >/dev/null 2>&1 || true
    fi
    set +e
    wait "${CURRENT_TCPDUMP_PID}" >/dev/null 2>&1
    set -e
    CURRENT_TCPDUMP_PID=""
  fi

  local playback_duration_ms=$((pull_end_ms - pull_start_ms))
  local playback_seconds=$((playback_duration_ms / 1000))

  if [[ ${startup_latency_ms} -lt 0 && -s "${pull_log}" ]]; then
    startup_latency_ms=$((pull_end_ms - pull_start_ms))
  fi

  local dts_in_pull=0
  local dts_in_push=0
  local freeze_events=0
  local dts_out_of_order=0
  local invalid_timestamps_in_pull=0
  local invalid_timestamps_in_push=0
  local non_increasing_dts_in_pull=0
  local non_increasing_dts_in_push=0
  local negative_cts_in_pull=0
  local negative_cts_in_push=0
  local invalid_timestamps=0
  local non_increasing_dts=0
  local negative_cts=0
  local first_second_avg_frame_interval_ms=-1
  local media_span_seconds=-1
  local average_playback_rate_x=-1
  local ffprobe_first_video_keyframe=1
  local ffprobe_first_video_pts_near_zero=1
  local ffprobe_video_dts_monotonic=1
  local first_keyframe_delay_ms=-1
  local source_repair_events=0
  local canonical_repair_events=0
  local egress_repair_events=0
  local repair_warn_high_frequency=0
  local repair_context_complete=1

  dts_in_pull=$(count_pattern_matches "${DTS_OUT_OF_ORDER_REGEX}" "${pull_log}")
  dts_in_push=$(count_pattern_matches "${DTS_OUT_OF_ORDER_REGEX}" "${push_log}")
  dts_out_of_order=$((dts_in_pull + dts_in_push))
  freeze_events=$(count_pattern_matches "${FREEZE_LOG_REGEX}" "${pull_log}")
  invalid_timestamps_in_pull=$(count_pattern_matches "${INVALID_TIMESTAMPS_REGEX}" "${pull_log}")
  invalid_timestamps_in_push=$(count_pattern_matches "${INVALID_TIMESTAMPS_REGEX}" "${push_log}")
  invalid_timestamps=$((invalid_timestamps_in_pull + invalid_timestamps_in_push))
  non_increasing_dts_in_pull=$(count_pattern_matches "${NON_INCREASING_DTS_REGEX}" "${pull_log}")
  non_increasing_dts_in_push=$(count_pattern_matches "${NON_INCREASING_DTS_REGEX}" "${push_log}")
  non_increasing_dts=$((non_increasing_dts_in_pull + non_increasing_dts_in_push))
  negative_cts_in_pull=$(count_pattern_matches "${NEGATIVE_CTS_REGEX}" "${pull_log}")
  negative_cts_in_push=$(count_pattern_matches "${NEGATIVE_CTS_REGEX}" "${push_log}")
  negative_cts=$((negative_cts_in_pull + negative_cts_in_push))
  first_second_avg_frame_interval_ms="$(extract_first_second_avg_interval_ms "${pull_log}")"
  media_span_seconds="$(extract_media_time_span_seconds "${pull_log}")"
  average_playback_rate_x="$(estimate_average_playback_rate_x "${media_span_seconds}" "${playback_duration_ms}")"
  source_repair_events=$(( $(count_pattern_matches "${SOURCE_REPAIR_REGEX}" "${pull_log}") + $(count_pattern_matches "${SOURCE_REPAIR_REGEX}" "${push_log}") ))
  canonical_repair_events=$(( $(count_pattern_matches "${CANONICAL_REPAIR_REGEX}" "${pull_log}") + $(count_pattern_matches "${CANONICAL_REPAIR_REGEX}" "${push_log}") ))
  egress_repair_events=$(( $(count_pattern_matches "${EGRESS_REPAIR_REGEX}" "${pull_log}") + $(count_pattern_matches "${EGRESS_REPAIR_REGEX}" "${push_log}") ))
  repair_context_complete="$(repair_context_complete_metric "${push_log}" "${pull_log}")"
  if (( canonical_repair_events > REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD || egress_repair_events > REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD )); then
    repair_warn_high_frequency=1
  fi
  if [[ "${ENABLE_FFPROBE_CHECKS}" == "1" ]]; then
    ffprobe_first_video_keyframe="$(ffprobe_first_video_keyframe_metric "${ffprobe_log}")"
    ffprobe_first_video_pts_near_zero="$(ffprobe_first_video_pts_near_zero_metric "${ffprobe_log}" "${FFPROBE_START_PTS_NEAR_ZERO_MS}")"
    ffprobe_video_dts_monotonic="$(ffprobe_video_dts_monotonic_metric "${ffprobe_log}")"
    first_keyframe_delay_ms="$(ffprobe_first_keyframe_delay_ms_metric "${ffprobe_log}")"
  fi

  {
    echo "dts_out_of_order=${dts_out_of_order} (pull=${dts_in_pull}, push=${dts_in_push})"
    echo "invalid_timestamps=${invalid_timestamps} (pull=${invalid_timestamps_in_pull}, push=${invalid_timestamps_in_push})"
    echo "non_increasing_dts=${non_increasing_dts} (pull=${non_increasing_dts_in_pull}, push=${non_increasing_dts_in_push})"
    echo "negative_cts=${negative_cts} (pull=${negative_cts_in_pull}, push=${negative_cts_in_push})"
    echo "ffprobe_first_video_keyframe=${ffprobe_first_video_keyframe}"
    echo "ffprobe_first_video_pts_near_zero=${ffprobe_first_video_pts_near_zero}"
    echo "ffprobe_video_dts_monotonic=${ffprobe_video_dts_monotonic}"
    echo "source_repair_events=${source_repair_events}"
    echo "canonical_repair_events=${canonical_repair_events}"
    echo "egress_repair_events=${egress_repair_events}"
    echo "repair_warn_high_frequency=${repair_warn_high_frequency}"
    echo "repair_context_complete=${repair_context_complete}"
    echo "ffprobe_exit_code=${ffprobe_rc}"
  } > "${anomaly_file}"

  local scenario_failed=0
  local failure_reasons=()

  if (( pull_rc != 0 && pull_rc != 124 )); then
    scenario_failed=1
    failure_reasons+=("pull command exited with non-acceptable code ${pull_rc}")
  fi

  local row
  while IFS= read -r row; do
    IFS='|' read -r check_key required threshold unit description <<< "${row}"
    [[ -n "${check_key}" && -n "${required}" && -n "${threshold}" && -n "${unit}" && -n "${description}" ]] || fail "invalid acceptance row: ${row}"
    [[ "${threshold}" =~ ^[0-9]+$ ]] || fail "invalid threshold '${threshold}' for check '${check_key}'"

    if ! is_required_check "${required}"; then
      continue
    fi

    local actual_value
    actual_value="$(metric_value "${check_key}" "${startup_latency_ms}" "${playback_seconds}" "${freeze_events}" "${dts_out_of_order}" "${invalid_timestamps}" "${non_increasing_dts}" "${negative_cts}" "${ffprobe_first_video_keyframe}" "${ffprobe_first_video_pts_near_zero}" "${ffprobe_video_dts_monotonic}" "${first_keyframe_delay_ms}" "${source_repair_events}" "${canonical_repair_events}" "${egress_repair_events}" "${repair_warn_high_frequency}" "${repair_context_complete}")"

    if ! compare_metric "${check_key}" "${actual_value}" "${threshold}"; then
      scenario_failed=1
      failure_reasons+=("${check_key} failed: actual=${actual_value} threshold=$(acceptance_operator "${check_key}") ${threshold} ${unit}")
    fi
  done < <(iter_acceptance_rows)

  {
    echo "scenario=${scenario}"
    echo "input_profile=${profile}"
    echo "pull_exit_code=${pull_rc}"
    echo "startup_latency_ms=${startup_latency_ms}"
    echo "playback_seconds=${playback_seconds}"
    echo "freeze_events=${freeze_events}"
    echo "dts_out_of_order=${dts_out_of_order}"
    echo "invalid_timestamps=${invalid_timestamps}"
    echo "non_increasing_dts=${non_increasing_dts}"
    echo "negative_cts=${negative_cts}"
    echo "ffprobe_first_video_keyframe=${ffprobe_first_video_keyframe}"
    echo "ffprobe_first_video_pts_near_zero=${ffprobe_first_video_pts_near_zero}"
    echo "ffprobe_video_dts_monotonic=${ffprobe_video_dts_monotonic}"
    echo "first_keyframe_delay_ms=${first_keyframe_delay_ms}"
    echo "source_repair_events=${source_repair_events}"
    echo "canonical_repair_events=${canonical_repair_events}"
    echo "egress_repair_events=${egress_repair_events}"
    echo "repair_warn_high_frequency=${repair_warn_high_frequency}"
    echo "repair_context_complete=${repair_context_complete}"
    echo "ffprobe_exit_code=${ffprobe_rc}"
    echo "first_second_avg_frame_interval_ms=${first_second_avg_frame_interval_ms}"
    echo "media_span_seconds=${media_span_seconds}"
    echo "average_playback_rate_x=${average_playback_rate_x}"
    echo "anomaly_summary=${anomaly_file}"
    echo "push_log=${push_log}"
    echo "pull_log=${pull_log}"
    echo "ffprobe_log=${ffprobe_log}"
    echo "pcap_file=${pcap_file}"
    if (( scenario_failed == 0 )); then
      echo "result=PASS"
    else
      echo "result=FAIL"
      printf 'reason=%s\n' "${failure_reasons[@]}"
      {
        echo "scenario=${scenario}"
        echo "input_profile=${profile}"
        echo "matrix_script=${MATRIX_SCRIPT}"
        echo "acceptance_file=${MATRIX_ACCEPTANCE_FILE}"
        echo "push_command=${push_cmd}"
        echo "pull_command=${pull_cmd}"
        echo "ffprobe_command=${ffprobe_cmd}"
      } > "${failure_input_file}"
      echo "failure_input=${failure_input_file}"
    fi
  } > "${summary_file}"

  if (( scenario_failed == 0 )); then
    log "scenario '${scenario}' passed (profile='${profile}')"
    return 0
  fi

  log "scenario '${scenario}' failed (profile='${profile}')"
  local reason
  for reason in "${failure_reasons[@]}"; do
    log "failure reason: ${reason}"
  done
  return 1
}

list_scenarios() {
  ensure_matrix_script
  "${MATRIX_SCRIPT}" list
}

run_one() {
  local scenario="$1"
  doctor

  local run_id
  run_id="$(date +%Y%m%d-%H%M%S)"
  mkdir -p "${REPORT_ROOT}/${run_id}"

  if run_scenario "${scenario}" "${INPUT_PROFILE}" "${run_id}"; then
    log "scenario report: ${REPORT_ROOT}/${run_id}/${INPUT_PROFILE}/${scenario}/summary.txt"
  else
    log "scenario report: ${REPORT_ROOT}/${run_id}/${INPUT_PROFILE}/${scenario}/summary.txt"
    fail "scenario '${scenario}' failed"
  fi
}

run_all() {
  doctor

  local run_id
  run_id="$(date +%Y%m%d-%H%M%S)"
  mkdir -p "${REPORT_ROOT}/${run_id}"

  local scenario
  local profile
  local failed=0
  local passed_count=0
  local failed_count=0

  while IFS= read -r profile; do
    [[ -n "${profile}" ]] || continue
    while IFS= read -r scenario; do
      [[ -n "${scenario}" ]] || continue

      if run_scenario "${scenario}" "${profile}" "${run_id}"; then
        passed_count=$((passed_count + 1))
      else
        failed=1
        failed_count=$((failed_count + 1))
      fi
    done < <(list_scenarios)
  done < <(list_profiles)

  log "run-all summary: passed=${passed_count}, failed=${failed_count}, report_root=${REPORT_ROOT}/${run_id}"

  if (( failed != 0 )); then
    fail "one or more scenarios failed"
  fi
}

main() {
  local command="${1:-}"

  case "${command}" in
    list)
      [[ $# -eq 1 ]] || fail "'list' takes no extra arguments"
      list_scenarios
      ;;
    run)
      [[ $# -eq 2 ]] || fail "'run' requires one scenario argument"
      run_one "$2"
      ;;
    run-all)
      [[ $# -eq 1 ]] || fail "'run-all' takes no extra arguments"
      run_all
      ;;
    doctor)
      [[ $# -eq 1 ]] || fail "'doctor' takes no extra arguments"
      doctor
      ;;
    -h|--help|help)
      usage
      ;;
    "")
      usage
      exit 1
      ;;
    *)
      fail "unknown command '${command}'. Run '${SCRIPT_NAME} --help' for usage."
      ;;
  esac
}

main "$@"
