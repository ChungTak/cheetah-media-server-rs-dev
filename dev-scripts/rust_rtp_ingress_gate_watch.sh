#!/usr/bin/env bash
set -euo pipefail

API_URL="${API_URL:-http://127.0.0.1:8891/api/v1/server/info}"
WINDOW_SECONDS="${WINDOW_SECONDS:-300}"
WINDOWS="${WINDOWS:-2}"
SPIKE_FACTOR="${SPIKE_FACTOR:-5}"
CURL_TIMEOUT="${CURL_TIMEOUT:-3}"
LOG_FILE="${LOG_FILE:-}"
ACK_KNOWN_ABNORMAL=0

SIP_METHOD_TIMEOUT_MAX_DELTA_USER_SET=0
SIP_CLIENT_TIMEOUT_MAX_DELTA_USER_SET=0
SIP_SERVER_TIMEOUT_MAX_DELTA_USER_SET=0
SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER_USER_SET=0
SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE_USER_SET=0
SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE_USER_SET=0
SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE_USER_SET=0
SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK_USER_SET=0
SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER_USER_SET=0
SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE_USER_SET=0
SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE_USER_SET=0
SIP_SERVER_TIMEOUT_MAX_DELTA_BYE_USER_SET=0
SIP_SERVER_TIMEOUT_MAX_DELTA_ACK_USER_SET=0
SIP_RUST_MAIN_FALLBACK_MAX_DELTA_USER_SET=0
SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA_USER_SET=0
SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA_USER_SET=0
SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA_USER_SET=0
SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA_USER_SET=0
SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA_USER_SET=0
SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA_USER_SET=0
SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA_USER_SET=0
SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA_USER_SET=0
SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA_USER_SET=0
SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA_USER_SET=0

if [ "${SIP_METHOD_TIMEOUT_MAX_DELTA+x}" = "x" ]; then SIP_METHOD_TIMEOUT_MAX_DELTA_USER_SET=1; fi
if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA+x}" = "x" ]; then SIP_CLIENT_TIMEOUT_MAX_DELTA_USER_SET=1; fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA+x}" = "x" ]; then SIP_SERVER_TIMEOUT_MAX_DELTA_USER_SET=1; fi
if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER+x}" = "x" ]; then SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER_USER_SET=1; fi
if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE+x}" = "x" ]; then SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE_USER_SET=1; fi
if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE+x}" = "x" ]; then SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE_USER_SET=1; fi
if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE+x}" = "x" ]; then SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE_USER_SET=1; fi
if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK+x}" = "x" ]; then SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK_USER_SET=1; fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER+x}" = "x" ]; then SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER_USER_SET=1; fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE+x}" = "x" ]; then SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE_USER_SET=1; fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE+x}" = "x" ]; then SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE_USER_SET=1; fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA_BYE+x}" = "x" ]; then SIP_SERVER_TIMEOUT_MAX_DELTA_BYE_USER_SET=1; fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA_ACK+x}" = "x" ]; then SIP_SERVER_TIMEOUT_MAX_DELTA_ACK_USER_SET=1; fi
if [ "${SIP_RUST_MAIN_FALLBACK_MAX_DELTA+x}" = "x" ]; then SIP_RUST_MAIN_FALLBACK_MAX_DELTA_USER_SET=1; fi
if [ "${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA+x}" = "x" ]; then SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA_USER_SET=1; fi
if [ "${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA+x}" = "x" ]; then SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA_USER_SET=1; fi
if [ "${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA+x}" = "x" ]; then SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA_USER_SET=1; fi
if [ "${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA+x}" = "x" ]; then SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA_USER_SET=1; fi
if [ "${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA+x}" = "x" ]; then SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA_USER_SET=1; fi
if [ "${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA+x}" = "x" ]; then SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA_USER_SET=1; fi
if [ "${SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA+x}" = "x" ]; then SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA_USER_SET=1; fi
if [ "${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA+x}" = "x" ]; then SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA_USER_SET=1; fi
if [ "${SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA+x}" = "x" ]; then SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA_USER_SET=1; fi
if [ "${SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA+x}" = "x" ]; then SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA_USER_SET=1; fi

SIP_METHOD_TIMEOUT_MAX_DELTA="${SIP_METHOD_TIMEOUT_MAX_DELTA:-0}"
SIP_CLIENT_TIMEOUT_MAX_DELTA="${SIP_CLIENT_TIMEOUT_MAX_DELTA:-0}"
SIP_SERVER_TIMEOUT_MAX_DELTA="${SIP_SERVER_TIMEOUT_MAX_DELTA:-0}"
SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER="${SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER:-0}"
SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE="${SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE:-0}"
SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE="${SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE:-0}"
SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE="${SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE:-0}"
SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK="${SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK:-0}"
SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER="${SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER:-0}"
SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE="${SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE:-0}"
SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE="${SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE:-0}"
SIP_SERVER_TIMEOUT_MAX_DELTA_BYE="${SIP_SERVER_TIMEOUT_MAX_DELTA_BYE:-0}"
SIP_SERVER_TIMEOUT_MAX_DELTA_ACK="${SIP_SERVER_TIMEOUT_MAX_DELTA_ACK:-0}"
SIP_METHOD_TIMEOUT_THRESHOLD_MODE="${SIP_METHOD_TIMEOUT_THRESHOLD_MODE:-absolute}"
SIP_METHOD_TIMEOUT_RATIO_BPS="${SIP_METHOD_TIMEOUT_RATIO_BPS:-0}"
SIP_METHOD_TIMEOUT_RATIO_MIN_TRAFFIC="${SIP_METHOD_TIMEOUT_RATIO_MIN_TRAFFIC:-0}"
SIP_RUST_MAIN_FALLBACK_MAX_DELTA="${SIP_RUST_MAIN_FALLBACK_MAX_DELTA:-0}"
SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA="${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA:-0}"
SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA="${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA:-0}"
SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA="${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA:-0}"
SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA="${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA:-0}"
SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA="${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA:-0}"
SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA="${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA:-0}"
SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA="${SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA:-0}"
SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA="${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA:-0}"
SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA="${SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA:-0}"
SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA="${SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA:-0}"

usage() {
    cat <<'USAGE'
Usage:
  rust_rtp_ingress_gate_watch.sh [options]

Options:
  --url <api_url>              Server info API URL (default: http://127.0.0.1:8891/api/v1/server/info)
  --window-seconds <seconds>   One window duration in seconds (default: 300)
  --windows <count>            Number of windows to observe (default: 2)
  --spike-factor <n>           Spike threshold vs previous window delta (default: 5)
  --curl-timeout <seconds>     Curl timeout for each snapshot request (default: 3)
  --log-file <path>            Optional server log path for mismatch warn count correlation
  --ack-known-abnormal         Acknowledge known abnormal traffic; skip "continuous growth" HOLD
  --sip-method-timeout-max-delta <n>
                               Default max delta for all method-level SIP timeout counters (default: 0)
  --sip-client-timeout-max-delta <n>
                               Max delta for all client method timeout counters (default: sip-method-timeout-max-delta)
  --sip-server-timeout-max-delta <n>
                               Max delta for all server method timeout counters (default: sip-method-timeout-max-delta)
  --sip-client-timeout-max-delta-{register,message,invite,bye} <n>
                               Per-method client timeout max delta override
  --sip-client-timeout-max-delta-ack <n>
                               Per-method client timeout max delta override
  --sip-server-timeout-max-delta-{register,message,invite,bye} <n>
                               Per-method server timeout max delta override
  --sip-server-timeout-max-delta-ack <n>
                               Per-method server timeout max delta override
  --sip-method-timeout-threshold-mode <absolute|normalized>
                               Method timeout threshold mode (default: absolute)
  --sip-method-timeout-ratio-bps <n>
                               Normalized mode ratio in basis points (1%=100, default: 0)
  --sip-method-timeout-ratio-min-traffic <n>
                               Normalized mode traffic floor per method delta (default: 0)
  --sip-rust-main-fallback-max-delta <n>
                               Optional default max delta for rust main fallback counters
  --sip-rust-main-precheck-drop-max-delta <n>
                               Optional max delta for rust main strict precheck drop counter
  --sip-rust-main-postparse-fallback-max-delta <n>
                               Optional max delta for rust main post-parse fallback counter
  --sip-rust-main-rescue-candidate-max-delta <n>
                               Optional max delta for rust main rescue-candidate counter
  --sip-rust-main-rust-invalid-fallback-max-delta <n>
                               Optional max delta for rust main rust-invalid-fallback counter
  --sip-rust-main-inject-rescue-fallback-max-delta <n>
                               Optional max delta for rust main inject rescue-fallback-synthetic counter
  --sip-rust-main-inject-invalid-fallback-max-delta <n>
                               Optional max delta for rust main inject invalid-fallback-synthetic counter
  --sip-rust-main-rescue-candidate-warn-delta <n>
                               Optional advisory threshold (no HOLD) for rescue-candidate delta
  --sip-rust-main-rust-invalid-fallback-warn-delta <n>
                               Optional advisory threshold (no HOLD) for rust-invalid-fallback delta
  --sip-outbound-complete-failed-max-delta <n>
                               Optional max delta for rustSipShadow.outboundCompleteClientTxFailed (disabled by default)
  --sip-outbound-complete-failed-warn-delta <n>
                               Advisory threshold for rustSipShadow.outboundCompleteClientTxFailed
                               (default when unset: advisory on any growth > 0)
  -h, --help                   Show this help

Observed counters:
  - rustRtpIngress.*.decisionMismatch
  - rustRuntime.selected.kind
  - rustRuntime.selected.udpCapable
  - rustSipShadow.mismatchPackets
  - rustSipShadow.rustMainRequested
  - rustSipShadow.rustMainEnabled
  - rustSipShadow.rustMainStrictEnabled
  - rustSipShadow.rustMainRescueEnabled
  - rustSipShadow.registerClientTxFailed
  - rustSipShadow.rollbackClientTx
  - rustSipShadow.outboundCompleteClientTxSuccess
  - rustSipShadow.outboundCompleteClientTxFailed
  - rustSipShadow.clientTxTimeouts
  - rustSipShadow.serverTxTimeouts
  - rustSipShadow.rustMainAttempts
  - rustSipShadow.rustMainSuccess
  - rustSipShadow.rustMainFallback
  - rustSipShadow.rustMainStrictDrops
  - rustSipShadow.rustMainPrecheckDrops
  - rustSipShadow.rustMainPostParseFallback
  - rustSipShadow.rustMainRescueCandidates
  - rustSipShadow.rustMainRustInvalidFallback
  - rustSipShadow.rustMainRescueParseAttempts
  - rustSipShadow.rustMainRescueParseSuccess
  - rustSipShadow.rustMainRescueParseFailed
  - rustSipShadow.injectRescueFallbackSynthetic
  - rustSipShadow.injectInvalidFallbackSynthetic
  - rustSipShadow.matchedClientResponses{Register,Message,Invite,Bye,Ack}
  - rustSipShadow.clientTxTimeouts{Register,Message,Invite,Bye,Ack}
  - rustSipShadow.serverTxTimeouts{Register,Message,Invite,Bye,Ack}

Exit code:
  0  PASS (can continue rollout)
  10 HOLD (must pause rollout and investigate)
  1  Runtime check error (API unavailable / response invalid)
  2  Argument/dependency error
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --url)
            API_URL="$2"; shift 2 ;;
        --window-seconds)
            WINDOW_SECONDS="$2"; shift 2 ;;
        --windows)
            WINDOWS="$2"; shift 2 ;;
        --spike-factor)
            SPIKE_FACTOR="$2"; shift 2 ;;
        --curl-timeout)
            CURL_TIMEOUT="$2"; shift 2 ;;
        --log-file)
            LOG_FILE="$2"; shift 2 ;;
        --ack-known-abnormal)
            ACK_KNOWN_ABNORMAL=1; shift ;;
        --sip-method-timeout-max-delta)
            SIP_METHOD_TIMEOUT_MAX_DELTA="$2"; SIP_METHOD_TIMEOUT_MAX_DELTA_USER_SET=1; shift 2 ;;
        --sip-client-timeout-max-delta)
            SIP_CLIENT_TIMEOUT_MAX_DELTA="$2"; SIP_CLIENT_TIMEOUT_MAX_DELTA_USER_SET=1; shift 2 ;;
        --sip-server-timeout-max-delta)
            SIP_SERVER_TIMEOUT_MAX_DELTA="$2"; SIP_SERVER_TIMEOUT_MAX_DELTA_USER_SET=1; shift 2 ;;
        --sip-client-timeout-max-delta-register)
            SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER="$2"; SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER_USER_SET=1; shift 2 ;;
        --sip-client-timeout-max-delta-message)
            SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE="$2"; SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE_USER_SET=1; shift 2 ;;
        --sip-client-timeout-max-delta-invite)
            SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE="$2"; SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE_USER_SET=1; shift 2 ;;
        --sip-client-timeout-max-delta-bye)
            SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE="$2"; SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE_USER_SET=1; shift 2 ;;
        --sip-client-timeout-max-delta-ack)
            SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK="$2"; SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK_USER_SET=1; shift 2 ;;
        --sip-server-timeout-max-delta-register)
            SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER="$2"; SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER_USER_SET=1; shift 2 ;;
        --sip-server-timeout-max-delta-message)
            SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE="$2"; SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE_USER_SET=1; shift 2 ;;
        --sip-server-timeout-max-delta-invite)
            SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE="$2"; SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE_USER_SET=1; shift 2 ;;
        --sip-server-timeout-max-delta-bye)
            SIP_SERVER_TIMEOUT_MAX_DELTA_BYE="$2"; SIP_SERVER_TIMEOUT_MAX_DELTA_BYE_USER_SET=1; shift 2 ;;
        --sip-server-timeout-max-delta-ack)
            SIP_SERVER_TIMEOUT_MAX_DELTA_ACK="$2"; SIP_SERVER_TIMEOUT_MAX_DELTA_ACK_USER_SET=1; shift 2 ;;
        --sip-method-timeout-threshold-mode)
            SIP_METHOD_TIMEOUT_THRESHOLD_MODE="$2"; shift 2 ;;
        --sip-method-timeout-ratio-bps)
            SIP_METHOD_TIMEOUT_RATIO_BPS="$2"; shift 2 ;;
        --sip-method-timeout-ratio-min-traffic)
            SIP_METHOD_TIMEOUT_RATIO_MIN_TRAFFIC="$2"; shift 2 ;;
        --sip-rust-main-fallback-max-delta)
            SIP_RUST_MAIN_FALLBACK_MAX_DELTA="$2"; SIP_RUST_MAIN_FALLBACK_MAX_DELTA_USER_SET=1; shift 2 ;;
        --sip-rust-main-precheck-drop-max-delta)
            SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA="$2"; SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA_USER_SET=1; shift 2 ;;
        --sip-rust-main-postparse-fallback-max-delta)
            SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA="$2"; SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA_USER_SET=1; shift 2 ;;
        --sip-rust-main-rescue-candidate-max-delta)
            SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA="$2"; SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA_USER_SET=1; shift 2 ;;
        --sip-rust-main-rust-invalid-fallback-max-delta)
            SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA="$2"; SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA_USER_SET=1; shift 2 ;;
        --sip-rust-main-inject-rescue-fallback-max-delta)
            SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA="$2"; SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA_USER_SET=1; shift 2 ;;
        --sip-rust-main-inject-invalid-fallback-max-delta)
            SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA="$2"; SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA_USER_SET=1; shift 2 ;;
        --sip-rust-main-rescue-candidate-warn-delta)
            SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA="$2"; SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA_USER_SET=1; shift 2 ;;
        --sip-rust-main-rust-invalid-fallback-warn-delta)
            SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA="$2"; SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA_USER_SET=1; shift 2 ;;
        --sip-outbound-complete-failed-max-delta)
            SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA="$2"; SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA_USER_SET=1; shift 2 ;;
        --sip-outbound-complete-failed-warn-delta)
            SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA="$2"; SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA_USER_SET=1; shift 2 ;;
        -h|--help)
            usage; exit 0 ;;
        *)
            echo "unknown option: $1" >&2
            usage
            exit 2 ;;
    esac
done

if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA_USER_SET}" -eq 0 ]; then
    SIP_CLIENT_TIMEOUT_MAX_DELTA="${SIP_METHOD_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA_USER_SET}" -eq 0 ]; then
    SIP_SERVER_TIMEOUT_MAX_DELTA="${SIP_METHOD_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER_USER_SET}" -eq 0 ]; then
    SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER="${SIP_CLIENT_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE_USER_SET}" -eq 0 ]; then
    SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE="${SIP_CLIENT_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE_USER_SET}" -eq 0 ]; then
    SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE="${SIP_CLIENT_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE_USER_SET}" -eq 0 ]; then
    SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE="${SIP_CLIENT_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK_USER_SET}" -eq 0 ]; then
    SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK="${SIP_CLIENT_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER_USER_SET}" -eq 0 ]; then
    SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER="${SIP_SERVER_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE_USER_SET}" -eq 0 ]; then
    SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE="${SIP_SERVER_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE_USER_SET}" -eq 0 ]; then
    SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE="${SIP_SERVER_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA_BYE_USER_SET}" -eq 0 ]; then
    SIP_SERVER_TIMEOUT_MAX_DELTA_BYE="${SIP_SERVER_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_SERVER_TIMEOUT_MAX_DELTA_ACK_USER_SET}" -eq 0 ]; then
    SIP_SERVER_TIMEOUT_MAX_DELTA_ACK="${SIP_SERVER_TIMEOUT_MAX_DELTA}"
fi
if [ "${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA_USER_SET}" -eq 0 ] \
    && [ "${SIP_RUST_MAIN_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ]; then
    SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA="${SIP_RUST_MAIN_FALLBACK_MAX_DELTA}"
    SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA_USER_SET=1
fi
if [ "${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA_USER_SET}" -eq 0 ] \
    && [ "${SIP_RUST_MAIN_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ]; then
    SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA="${SIP_RUST_MAIN_FALLBACK_MAX_DELTA}"
    SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA_USER_SET=1
fi
if [ "${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA_USER_SET}" -eq 0 ] \
    && [ "${SIP_RUST_MAIN_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ]; then
    SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA="${SIP_RUST_MAIN_FALLBACK_MAX_DELTA}"
    SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA_USER_SET=1
fi
if [ "${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA_USER_SET}" -eq 0 ] \
    && [ "${SIP_RUST_MAIN_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ]; then
    SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA="${SIP_RUST_MAIN_FALLBACK_MAX_DELTA}"
    SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA_USER_SET=1
fi
if [ "${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA_USER_SET}" -eq 0 ] \
    && [ "${SIP_RUST_MAIN_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ]; then
    SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA="${SIP_RUST_MAIN_FALLBACK_MAX_DELTA}"
    SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA_USER_SET=1
fi
if [ "${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA_USER_SET}" -eq 0 ] \
    && [ "${SIP_RUST_MAIN_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ]; then
    SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA="${SIP_RUST_MAIN_FALLBACK_MAX_DELTA}"
    SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA_USER_SET=1
fi

if ! command -v curl >/dev/null 2>&1; then
    echo "curl not found" >&2
    exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "jq not found" >&2
    exit 2
fi

if ! [[ "${WINDOW_SECONDS}" =~ ^[0-9]+$ ]] || [ "${WINDOW_SECONDS}" -le 0 ]; then
    echo "window-seconds must be a positive integer" >&2
    exit 2
fi
if ! [[ "${WINDOWS}" =~ ^[0-9]+$ ]] || [ "${WINDOWS}" -le 0 ]; then
    echo "windows must be a positive integer" >&2
    exit 2
fi
if ! [[ "${SPIKE_FACTOR}" =~ ^[0-9]+$ ]] || [ "${SPIKE_FACTOR}" -le 1 ]; then
    echo "spike-factor must be an integer >= 2" >&2
    exit 2
fi
if ! [[ "${CURL_TIMEOUT}" =~ ^[0-9]+$ ]] || [ "${CURL_TIMEOUT}" -le 0 ]; then
    echo "curl-timeout must be a positive integer" >&2
    exit 2
fi
if [ "${SIP_METHOD_TIMEOUT_THRESHOLD_MODE}" != "absolute" ] && [ "${SIP_METHOD_TIMEOUT_THRESHOLD_MODE}" != "normalized" ]; then
    echo "sip-method-timeout-threshold-mode must be one of: absolute, normalized" >&2
    exit 2
fi
for kv in \
    "sip-method-timeout-max-delta:${SIP_METHOD_TIMEOUT_MAX_DELTA}" \
    "sip-client-timeout-max-delta:${SIP_CLIENT_TIMEOUT_MAX_DELTA}" \
    "sip-server-timeout-max-delta:${SIP_SERVER_TIMEOUT_MAX_DELTA}" \
    "sip-client-timeout-max-delta-register:${SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER}" \
    "sip-client-timeout-max-delta-message:${SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE}" \
    "sip-client-timeout-max-delta-invite:${SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE}" \
    "sip-client-timeout-max-delta-bye:${SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE}" \
    "sip-client-timeout-max-delta-ack:${SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK}" \
    "sip-server-timeout-max-delta-register:${SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER}" \
    "sip-server-timeout-max-delta-message:${SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE}" \
    "sip-server-timeout-max-delta-invite:${SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE}" \
    "sip-server-timeout-max-delta-bye:${SIP_SERVER_TIMEOUT_MAX_DELTA_BYE}" \
    "sip-server-timeout-max-delta-ack:${SIP_SERVER_TIMEOUT_MAX_DELTA_ACK}" \
    "sip-method-timeout-ratio-bps:${SIP_METHOD_TIMEOUT_RATIO_BPS}" \
    "sip-method-timeout-ratio-min-traffic:${SIP_METHOD_TIMEOUT_RATIO_MIN_TRAFFIC}"; do
    key="${kv%%:*}"
    val="${kv#*:}"
    if ! [[ "${val}" =~ ^[0-9]+$ ]]; then
        echo "${key} must be a non-negative integer" >&2
        exit 2
    fi
done
for kv in \
    "sip-rust-main-fallback-max-delta:${SIP_RUST_MAIN_FALLBACK_MAX_DELTA}:${SIP_RUST_MAIN_FALLBACK_MAX_DELTA_USER_SET}" \
    "sip-rust-main-precheck-drop-max-delta:${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA}:${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA_USER_SET}" \
    "sip-rust-main-postparse-fallback-max-delta:${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA}:${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA_USER_SET}" \
    "sip-rust-main-rescue-candidate-max-delta:${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA}:${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA_USER_SET}" \
    "sip-rust-main-rust-invalid-fallback-max-delta:${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA}:${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA_USER_SET}" \
    "sip-rust-main-inject-rescue-fallback-max-delta:${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA}:${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA_USER_SET}" \
    "sip-rust-main-inject-invalid-fallback-max-delta:${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA}:${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA_USER_SET}" \
    "sip-rust-main-rescue-candidate-warn-delta:${SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA}:${SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA_USER_SET}" \
    "sip-rust-main-rust-invalid-fallback-warn-delta:${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA}:${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA_USER_SET}" \
    "sip-outbound-complete-failed-max-delta:${SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA}:${SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA_USER_SET}" \
    "sip-outbound-complete-failed-warn-delta:${SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA}:${SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA_USER_SET}"; do
    key="${kv%%:*}"
    rem="${kv#*:}"
    val="${rem%%:*}"
    enabled="${rem##*:}"
    if [ "${enabled}" -eq 1 ] && ! [[ "${val}" =~ ^[0-9]+$ ]]; then
        echo "${key} must be a non-negative integer" >&2
        exit 2
    fi
done
if [ -n "${LOG_FILE}" ] && [ ! -f "${LOG_FILE}" ]; then
    echo "log file not found: ${LOG_FILE}" >&2
    exit 2
fi

fetch_snapshot() {
    local tmp
    tmp="$(mktemp)"
    if ! curl -fsS -m "${CURL_TIMEOUT}" "${API_URL}" > "${tmp}"; then
        rm -f "${tmp}"
        echo "failed to fetch ${API_URL}" >&2
        return 1
    fi

    local enabled total rtp gb conn
    local sip_enabled sip_enabled_num sip_mismatch sip_reg_fail sip_rollback sip_cto sip_sto
    local sip_outbound_complete_success sip_outbound_complete_failed
    local sip_rmain_enabled sip_rmain_strict_enabled sip_rmain_rescue_enabled
    local sip_rmain_enabled_num sip_rmain_strict_enabled_num sip_rmain_rescue_enabled_num
    local sip_rmain_attempt sip_rmain_success sip_rmain_fallback sip_rmain_strict_drop
    local sip_rmain_precheck_drop sip_rmain_postparse_fallback sip_rmain_rescue sip_rmain_rust_invalid
    local sip_rmain_rescue_parse_attempt sip_rmain_rescue_parse_success sip_rmain_rescue_parse_failed
    local sip_inject_rescue_fallback sip_inject_invalid_fallback
    local sip_mcr_reg sip_mcr_msg sip_mcr_inv sip_mcr_bye sip_mcr_ack
    local sip_cto_reg sip_cto_msg sip_cto_inv sip_cto_bye sip_cto_ack
    local sip_sto_reg sip_sto_msg sip_sto_inv sip_sto_bye sip_sto_ack
    enabled="$(jq -r '.rustRtpIngress.enabled // false' "${tmp}")"
    if [ "${enabled}" != "true" ]; then
        rm -f "${tmp}"
        echo "rustRtpIngress.enabled is false (or missing), cannot run gate watch" >&2
        return 1
    fi

    total="$(jq -r '.rustRtpIngress.total.decisionMismatch // 0' "${tmp}")"
    rtp="$(jq -r '.rustRtpIngress.rtpServer.decisionMismatch // 0' "${tmp}")"
    gb="$(jq -r '.rustRtpIngress.gb28181.decisionMismatch // 0' "${tmp}")"
    conn="$(jq -r '.rustRtpIngress.rtpConnection.decisionMismatch // 0' "${tmp}")"

    sip_enabled="$(jq -r '.rustSipShadow.enabled // false' "${tmp}")"
    if [ "${sip_enabled}" = "true" ]; then
        sip_enabled_num=1
        sip_rmain_enabled="$(jq -r '.rustSipShadow.rustMainEnabled // false' "${tmp}")"
        sip_rmain_strict_enabled="$(jq -r '.rustSipShadow.rustMainStrictEnabled // false' "${tmp}")"
        sip_rmain_rescue_enabled="$(jq -r '.rustSipShadow.rustMainRescueEnabled // false' "${tmp}")"
        if [ "${sip_rmain_enabled}" = "true" ]; then
            sip_rmain_enabled_num=1
        else
            sip_rmain_enabled_num=0
        fi
        if [ "${sip_rmain_strict_enabled}" = "true" ]; then
            sip_rmain_strict_enabled_num=1
        else
            sip_rmain_strict_enabled_num=0
        fi
        if [ "${sip_rmain_rescue_enabled}" = "true" ]; then
            sip_rmain_rescue_enabled_num=1
        else
            sip_rmain_rescue_enabled_num=0
        fi
        sip_mismatch="$(jq -r '.rustSipShadow.mismatchPackets // 0' "${tmp}")"
        sip_reg_fail="$(jq -r '.rustSipShadow.registerClientTxFailed // 0' "${tmp}")"
        sip_rollback="$(jq -r '.rustSipShadow.rollbackClientTx // 0' "${tmp}")"
        sip_outbound_complete_success="$(jq -r '.rustSipShadow.outboundCompleteClientTxSuccess // 0' "${tmp}")"
        sip_outbound_complete_failed="$(jq -r '.rustSipShadow.outboundCompleteClientTxFailed // 0' "${tmp}")"
        sip_cto="$(jq -r '.rustSipShadow.clientTxTimeouts // 0' "${tmp}")"
        sip_sto="$(jq -r '.rustSipShadow.serverTxTimeouts // 0' "${tmp}")"
        sip_rmain_attempt="$(jq -r '.rustSipShadow.rustMainAttempts // 0' "${tmp}")"
        sip_rmain_success="$(jq -r '.rustSipShadow.rustMainSuccess // 0' "${tmp}")"
        sip_rmain_fallback="$(jq -r '.rustSipShadow.rustMainFallback // 0' "${tmp}")"
        sip_rmain_strict_drop="$(jq -r '.rustSipShadow.rustMainStrictDrops // 0' "${tmp}")"
        sip_rmain_precheck_drop="$(jq -r '.rustSipShadow.rustMainPrecheckDrops // 0' "${tmp}")"
        sip_rmain_postparse_fallback="$(jq -r '.rustSipShadow.rustMainPostParseFallback // 0' "${tmp}")"
        sip_rmain_rescue="$(jq -r '.rustSipShadow.rustMainRescueCandidates // 0' "${tmp}")"
        sip_rmain_rust_invalid="$(jq -r '.rustSipShadow.rustMainRustInvalidFallback // 0' "${tmp}")"
        sip_rmain_rescue_parse_attempt="$(jq -r '.rustSipShadow.rustMainRescueParseAttempts // 0' "${tmp}")"
        sip_rmain_rescue_parse_success="$(jq -r '.rustSipShadow.rustMainRescueParseSuccess // 0' "${tmp}")"
        sip_rmain_rescue_parse_failed="$(jq -r '.rustSipShadow.rustMainRescueParseFailed // 0' "${tmp}")"
        sip_inject_rescue_fallback="$(jq -r '.rustSipShadow.injectRescueFallbackSynthetic // 0' "${tmp}")"
        sip_inject_invalid_fallback="$(jq -r '.rustSipShadow.injectInvalidFallbackSynthetic // 0' "${tmp}")"
        sip_mcr_reg="$(jq -r '.rustSipShadow.matchedClientResponsesRegister // 0' "${tmp}")"
        sip_mcr_msg="$(jq -r '.rustSipShadow.matchedClientResponsesMessage // 0' "${tmp}")"
        sip_mcr_inv="$(jq -r '.rustSipShadow.matchedClientResponsesInvite // 0' "${tmp}")"
        sip_mcr_bye="$(jq -r '.rustSipShadow.matchedClientResponsesBye // 0' "${tmp}")"
        sip_mcr_ack="$(jq -r '.rustSipShadow.matchedClientResponsesAck // 0' "${tmp}")"
        sip_cto_reg="$(jq -r '.rustSipShadow.clientTxTimeoutsRegister // 0' "${tmp}")"
        sip_cto_msg="$(jq -r '.rustSipShadow.clientTxTimeoutsMessage // 0' "${tmp}")"
        sip_cto_inv="$(jq -r '.rustSipShadow.clientTxTimeoutsInvite // 0' "${tmp}")"
        sip_cto_bye="$(jq -r '.rustSipShadow.clientTxTimeoutsBye // 0' "${tmp}")"
        sip_cto_ack="$(jq -r '.rustSipShadow.clientTxTimeoutsAck // 0' "${tmp}")"
        sip_sto_reg="$(jq -r '.rustSipShadow.serverTxTimeoutsRegister // 0' "${tmp}")"
        sip_sto_msg="$(jq -r '.rustSipShadow.serverTxTimeoutsMessage // 0' "${tmp}")"
        sip_sto_inv="$(jq -r '.rustSipShadow.serverTxTimeoutsInvite // 0' "${tmp}")"
        sip_sto_bye="$(jq -r '.rustSipShadow.serverTxTimeoutsBye // 0' "${tmp}")"
        sip_sto_ack="$(jq -r '.rustSipShadow.serverTxTimeoutsAck // 0' "${tmp}")"
    else
        sip_enabled_num=0
        sip_rmain_enabled_num=0
        sip_rmain_strict_enabled_num=0
        sip_rmain_rescue_enabled_num=0
        sip_mismatch=0
        sip_reg_fail=0
        sip_rollback=0
        sip_outbound_complete_success=0
        sip_outbound_complete_failed=0
        sip_cto=0
        sip_sto=0
        sip_rmain_attempt=0
        sip_rmain_success=0
        sip_rmain_fallback=0
        sip_rmain_strict_drop=0
        sip_rmain_precheck_drop=0
        sip_rmain_postparse_fallback=0
        sip_rmain_rescue=0
        sip_rmain_rust_invalid=0
        sip_rmain_rescue_parse_attempt=0
        sip_rmain_rescue_parse_success=0
        sip_rmain_rescue_parse_failed=0
        sip_inject_rescue_fallback=0
        sip_inject_invalid_fallback=0
        sip_mcr_reg=0
        sip_mcr_msg=0
        sip_mcr_inv=0
        sip_mcr_bye=0
        sip_mcr_ack=0
        sip_cto_reg=0
        sip_cto_msg=0
        sip_cto_inv=0
        sip_cto_bye=0
        sip_cto_ack=0
        sip_sto_reg=0
        sip_sto_msg=0
        sip_sto_inv=0
        sip_sto_bye=0
        sip_sto_ack=0
    fi
    rm -f "${tmp}"

    if ! [[ "${total}" =~ ^[0-9]+$ && "${rtp}" =~ ^[0-9]+$ && "${gb}" =~ ^[0-9]+$ && "${conn}" =~ ^[0-9]+$ \
        && "${sip_mismatch}" =~ ^[0-9]+$ && "${sip_reg_fail}" =~ ^[0-9]+$ && "${sip_rollback}" =~ ^[0-9]+$ \
        && "${sip_outbound_complete_success}" =~ ^[0-9]+$ && "${sip_outbound_complete_failed}" =~ ^[0-9]+$ \
        && "${sip_cto}" =~ ^[0-9]+$ && "${sip_sto}" =~ ^[0-9]+$ \
        && "${sip_rmain_enabled_num}" =~ ^[0-9]+$ && "${sip_rmain_strict_enabled_num}" =~ ^[0-9]+$ && "${sip_rmain_rescue_enabled_num}" =~ ^[0-9]+$ \
        && "${sip_rmain_attempt}" =~ ^[0-9]+$ && "${sip_rmain_success}" =~ ^[0-9]+$ && "${sip_rmain_fallback}" =~ ^[0-9]+$ && "${sip_rmain_strict_drop}" =~ ^[0-9]+$ \
        && "${sip_rmain_precheck_drop}" =~ ^[0-9]+$ && "${sip_rmain_postparse_fallback}" =~ ^[0-9]+$ \
        && "${sip_rmain_rescue}" =~ ^[0-9]+$ && "${sip_rmain_rust_invalid}" =~ ^[0-9]+$ \
        && "${sip_rmain_rescue_parse_attempt}" =~ ^[0-9]+$ && "${sip_rmain_rescue_parse_success}" =~ ^[0-9]+$ && "${sip_rmain_rescue_parse_failed}" =~ ^[0-9]+$ \
        && "${sip_inject_rescue_fallback}" =~ ^[0-9]+$ && "${sip_inject_invalid_fallback}" =~ ^[0-9]+$ \
        && "${sip_mcr_reg}" =~ ^[0-9]+$ && "${sip_mcr_msg}" =~ ^[0-9]+$ && "${sip_mcr_inv}" =~ ^[0-9]+$ && "${sip_mcr_bye}" =~ ^[0-9]+$ && "${sip_mcr_ack}" =~ ^[0-9]+$ \
        && "${sip_cto_reg}" =~ ^[0-9]+$ && "${sip_cto_msg}" =~ ^[0-9]+$ && "${sip_cto_inv}" =~ ^[0-9]+$ && "${sip_cto_bye}" =~ ^[0-9]+$ && "${sip_cto_ack}" =~ ^[0-9]+$ \
        && "${sip_sto_reg}" =~ ^[0-9]+$ && "${sip_sto_msg}" =~ ^[0-9]+$ && "${sip_sto_inv}" =~ ^[0-9]+$ && "${sip_sto_bye}" =~ ^[0-9]+$ && "${sip_sto_ack}" =~ ^[0-9]+$ ]]; then
        echo "invalid decisionMismatch fields from API response" >&2
        return 1
    fi
    echo "${total} ${rtp} ${gb} ${conn} ${sip_mismatch} ${sip_reg_fail} ${sip_rollback} ${sip_outbound_complete_success} ${sip_outbound_complete_failed} ${sip_cto} ${sip_sto} ${sip_enabled_num} ${sip_rmain_enabled_num} ${sip_rmain_strict_enabled_num} ${sip_rmain_rescue_enabled_num} ${sip_rmain_attempt} ${sip_rmain_success} ${sip_rmain_fallback} ${sip_rmain_strict_drop} ${sip_rmain_precheck_drop} ${sip_rmain_postparse_fallback} ${sip_rmain_rescue} ${sip_rmain_rust_invalid} ${sip_rmain_rescue_parse_attempt} ${sip_rmain_rescue_parse_success} ${sip_rmain_rescue_parse_failed} ${sip_inject_rescue_fallback} ${sip_inject_invalid_fallback} \
${sip_mcr_reg} ${sip_mcr_msg} ${sip_mcr_inv} ${sip_mcr_bye} ${sip_mcr_ack} \
${sip_cto_reg} ${sip_cto_msg} ${sip_cto_inv} ${sip_cto_bye} ${sip_cto_ack} \
${sip_sto_reg} ${sip_sto_msg} ${sip_sto_inv} ${sip_sto_bye} ${sip_sto_ack}"
}

preflight_runtime_capability_gate() {
    local tmp
    tmp="$(mktemp)"
    if ! curl -fsS -m "${CURL_TIMEOUT}" "${API_URL}" > "${tmp}"; then
        rm -f "${tmp}"
        echo "failed to fetch ${API_URL} for runtime capability preflight" >&2
        return 1
    fi

    local runtime_enabled runtime_kind runtime_udp_capable sip_enabled sip_rmain_requested sip_rmain_enabled sip_rmain_rescue_enabled
    runtime_enabled="$(jq -r '.rustRuntime.enabled // false' "${tmp}")"
    runtime_kind="$(jq -r '.rustRuntime.selected.kind // "unknown"' "${tmp}")"
    runtime_udp_capable="$(jq -r '.rustRuntime.selected.udpCapable // false' "${tmp}")"
    sip_enabled="$(jq -r '.rustSipShadow.enabled // false' "${tmp}")"
    sip_rmain_requested="$(jq -r '.rustSipShadow.rustMainRequested // false' "${tmp}")"
    sip_rmain_enabled="$(jq -r '.rustSipShadow.rustMainEnabled // false' "${tmp}")"
    sip_rmain_rescue_enabled="$(jq -r '.rustSipShadow.rustMainRescueEnabled // false' "${tmp}")"
    rm -f "${tmp}"

    if [ "${runtime_enabled}" = "true" ] \
        && [ "${sip_enabled}" = "true" ] \
        && [ "${sip_rmain_requested}" = "true" ] \
        && [ "${runtime_udp_capable}" != "true" ]; then
        echo ""
        echo "===== Rust Ingress Gate Summary ====="
        echo "api_url=${API_URL}"
        echo "rust_runtime={enabled:${runtime_enabled},kind:${runtime_kind},udpCapable:${runtime_udp_capable}}"
        echo "rust_sip_shadow={enabled:${sip_enabled},rustMainRequested:${sip_rmain_requested},rustMainEnabled:${sip_rmain_enabled},rustMainRescueEnabled:${sip_rmain_rescue_enabled}}"
        echo "reason: runtime capability violation: selected runtime is not udp-capable while rustSipShadow.rustMainRequested=true"
        echo "result=HOLD"
        return 10
    fi
    return 0
}

set +e
preflight_runtime_capability_gate
preflight_rc=$?
set -e
if [ "${preflight_rc}" -eq 10 ]; then
    exit 10
fi
if [ "${preflight_rc}" -ne 0 ]; then
    exit 1
fi

current_warn_count() {
    if [ -z "${LOG_FILE}" ]; then
        echo "-1"
        return 0
    fi
    rg -c "rust/legacy decision mismatch" "${LOG_FILE}" || true
}

compute_method_timeout_limit() {
    local absolute_limit="$1"
    local method_traffic="$2"
    local limit="${absolute_limit}"
    if [ "${SIP_METHOD_TIMEOUT_THRESHOLD_MODE}" = "normalized" ]; then
        local traffic="${method_traffic}"
        if [ "${traffic}" -lt "${SIP_METHOD_TIMEOUT_RATIO_MIN_TRAFFIC}" ]; then
            traffic="${SIP_METHOD_TIMEOUT_RATIO_MIN_TRAFFIC}"
        fi
        local extra=$(( (traffic * SIP_METHOD_TIMEOUT_RATIO_BPS + 9999) / 10000 ))
        limit=$(( absolute_limit + extra ))
    fi
    echo "${limit}"
}

declare -a ts_arr=()
declare -a total_arr=()
declare -a rtp_arr=()
declare -a gb_arr=()
declare -a conn_arr=()
declare -a sip_mismatch_arr=()
declare -a sip_reg_fail_arr=()
declare -a sip_rollback_arr=()
declare -a sip_outbound_complete_success_arr=()
declare -a sip_outbound_complete_failed_arr=()
declare -a sip_cto_arr=()
declare -a sip_sto_arr=()
declare -a sip_rmain_attempt_arr=()
declare -a sip_rmain_success_arr=()
declare -a sip_rmain_fallback_arr=()
declare -a sip_rmain_strict_drop_arr=()
declare -a sip_rmain_precheck_drop_arr=()
declare -a sip_rmain_postparse_fallback_arr=()
declare -a sip_rmain_rescue_arr=()
declare -a sip_rmain_rust_invalid_arr=()
declare -a sip_inject_rescue_fallback_arr=()
declare -a sip_inject_invalid_fallback_arr=()
declare -a sip_rmain_enabled_arr=()
declare -a sip_rmain_strict_enabled_arr=()
declare -a sip_rmain_rescue_enabled_arr=()
declare -a sip_rmain_rescue_parse_attempt_arr=()
declare -a sip_rmain_rescue_parse_success_arr=()
declare -a sip_rmain_rescue_parse_failed_arr=()
declare -a sip_mcr_reg_arr=()
declare -a sip_mcr_msg_arr=()
declare -a sip_mcr_inv_arr=()
declare -a sip_mcr_bye_arr=()
declare -a sip_mcr_ack_arr=()
declare -a sip_cto_reg_arr=()
declare -a sip_cto_msg_arr=()
declare -a sip_cto_inv_arr=()
declare -a sip_cto_bye_arr=()
declare -a sip_cto_ack_arr=()
declare -a sip_sto_reg_arr=()
declare -a sip_sto_msg_arr=()
declare -a sip_sto_inv_arr=()
declare -a sip_sto_bye_arr=()
declare -a sip_sto_ack_arr=()
declare -a sip_enabled_arr=()
declare -a warn_arr=()

read -r t0 r0 g0 c0 sm0 srf0 srb0 socs0 socf0 sct0 sst0 se0 srme0 srms0 srmre0 sra0 srs0 srfb0 srsd0 srmpd0 srmppf0 srmrc0 srmri0 srmrpa0 srmrps0 srmrpf0 sirf0 siif0 \
    mcrr0 mcrm0 mcri0 mcrb0 mcra0 \
    sctr0 sctm0 scti0 sctb0 scta0 \
    sstr0 sstm0 ssti0 sstb0 ssta0 < <(fetch_snapshot)
ts_arr+=("$(date '+%Y-%m-%d %H:%M:%S')")
total_arr+=("${t0}")
rtp_arr+=("${r0}")
gb_arr+=("${g0}")
conn_arr+=("${c0}")
sip_mismatch_arr+=("${sm0}")
sip_reg_fail_arr+=("${srf0}")
sip_rollback_arr+=("${srb0}")
sip_outbound_complete_success_arr+=("${socs0}")
sip_outbound_complete_failed_arr+=("${socf0}")
sip_cto_arr+=("${sct0}")
sip_sto_arr+=("${sst0}")
sip_rmain_enabled_arr+=("${srme0}")
sip_rmain_strict_enabled_arr+=("${srms0}")
sip_rmain_rescue_enabled_arr+=("${srmre0}")
sip_rmain_attempt_arr+=("${sra0}")
sip_rmain_success_arr+=("${srs0}")
sip_rmain_fallback_arr+=("${srfb0}")
sip_rmain_strict_drop_arr+=("${srsd0}")
sip_rmain_precheck_drop_arr+=("${srmpd0}")
sip_rmain_postparse_fallback_arr+=("${srmppf0}")
sip_rmain_rescue_arr+=("${srmrc0}")
sip_rmain_rust_invalid_arr+=("${srmri0}")
sip_rmain_rescue_parse_attempt_arr+=("${srmrpa0}")
sip_rmain_rescue_parse_success_arr+=("${srmrps0}")
sip_rmain_rescue_parse_failed_arr+=("${srmrpf0}")
sip_inject_rescue_fallback_arr+=("${sirf0}")
sip_inject_invalid_fallback_arr+=("${siif0}")
sip_mcr_reg_arr+=("${mcrr0}")
sip_mcr_msg_arr+=("${mcrm0}")
sip_mcr_inv_arr+=("${mcri0}")
sip_mcr_bye_arr+=("${mcrb0}")
sip_mcr_ack_arr+=("${mcra0}")
sip_cto_reg_arr+=("${sctr0}")
sip_cto_msg_arr+=("${sctm0}")
sip_cto_inv_arr+=("${scti0}")
sip_cto_bye_arr+=("${sctb0}")
sip_cto_ack_arr+=("${scta0}")
sip_sto_reg_arr+=("${sstr0}")
sip_sto_msg_arr+=("${sstm0}")
sip_sto_inv_arr+=("${ssti0}")
sip_sto_bye_arr+=("${sstb0}")
sip_sto_ack_arr+=("${ssta0}")
sip_enabled_arr+=("${se0}")
warn_arr+=("$(current_warn_count)")

echo "[gate] baseline: ts=${ts_arr[0]} total=${t0} rtp=${r0} gb28181=${g0} rtpConnection=${c0} sipMismatch=${sm0} sipRegFail=${srf0} sipRollback=${srb0} sipOutboundComplete={success:${socs0},failed:${socf0}} sipClientTimeout=${sct0} sipServerTimeout=${sst0} sipEnabled=${se0} \
rustMain={enabled:${srme0},strictEnabled:${srms0},rescueEnabled:${srmre0},attempt:${sra0},success:${srs0},fallback:${srfb0},strictDrop:${srsd0},precheckDrop:${srmpd0},postParseFallback:${srmppf0},rescueCandidate:${srmrc0},rustInvalidFallback:${srmri0},rescueParseAttempt:${srmrpa0},rescueParseSuccess:${srmrps0},rescueParseFailed:${srmrpf0},injectRescueFallbackSynthetic:${sirf0},injectInvalidFallbackSynthetic:${siif0}} \
matchedByMethod={register:${mcrr0},message:${mcrm0},invite:${mcri0},bye:${mcrb0},ack:${mcra0}} \
clientTimeoutByMethod={register:${sctr0},message:${sctm0},invite:${scti0},bye:${sctb0},ack:${scta0}} \
serverTimeoutByMethod={register:${sstr0},message:${sstm0},invite:${ssti0},bye:${sstb0},ack:${ssta0}} warn=${warn_arr[0]}"

for i in $(seq 1 "${WINDOWS}"); do
    sleep "${WINDOW_SECONDS}"
    read -r t r g c sm srf srb socs socf sct sst se srme srms srmre sra srs srfb srsd srmpd srmppf srmrc srmri srmrpa srmrps srmrpf sirf siif \
        mcrr mcrm mcri mcrb mcra \
        sctr sctm scti sctb scta \
        sstr sstm ssti sstb ssta < <(fetch_snapshot)
    ts_arr+=("$(date '+%Y-%m-%d %H:%M:%S')")
    total_arr+=("${t}")
    rtp_arr+=("${r}")
    gb_arr+=("${g}")
    conn_arr+=("${c}")
    sip_mismatch_arr+=("${sm}")
    sip_reg_fail_arr+=("${srf}")
    sip_rollback_arr+=("${srb}")
    sip_outbound_complete_success_arr+=("${socs}")
    sip_outbound_complete_failed_arr+=("${socf}")
    sip_cto_arr+=("${sct}")
    sip_sto_arr+=("${sst}")
    sip_rmain_enabled_arr+=("${srme}")
    sip_rmain_strict_enabled_arr+=("${srms}")
    sip_rmain_rescue_enabled_arr+=("${srmre}")
    sip_rmain_attempt_arr+=("${sra}")
    sip_rmain_success_arr+=("${srs}")
    sip_rmain_fallback_arr+=("${srfb}")
    sip_rmain_strict_drop_arr+=("${srsd}")
    sip_rmain_precheck_drop_arr+=("${srmpd}")
    sip_rmain_postparse_fallback_arr+=("${srmppf}")
    sip_rmain_rescue_arr+=("${srmrc}")
    sip_rmain_rust_invalid_arr+=("${srmri}")
    sip_rmain_rescue_parse_attempt_arr+=("${srmrpa}")
    sip_rmain_rescue_parse_success_arr+=("${srmrps}")
    sip_rmain_rescue_parse_failed_arr+=("${srmrpf}")
    sip_inject_rescue_fallback_arr+=("${sirf}")
    sip_inject_invalid_fallback_arr+=("${siif}")
    sip_mcr_reg_arr+=("${mcrr}")
    sip_mcr_msg_arr+=("${mcrm}")
    sip_mcr_inv_arr+=("${mcri}")
    sip_mcr_bye_arr+=("${mcrb}")
    sip_mcr_ack_arr+=("${mcra}")
    sip_cto_reg_arr+=("${sctr}")
    sip_cto_msg_arr+=("${sctm}")
    sip_cto_inv_arr+=("${scti}")
    sip_cto_bye_arr+=("${sctb}")
    sip_cto_ack_arr+=("${scta}")
    sip_sto_reg_arr+=("${sstr}")
    sip_sto_msg_arr+=("${sstm}")
    sip_sto_inv_arr+=("${ssti}")
    sip_sto_bye_arr+=("${sstb}")
    sip_sto_ack_arr+=("${ssta}")
    sip_enabled_arr+=("${se}")
    warn_arr+=("$(current_warn_count)")
    echo "[gate] snapshot#${i}: ts=${ts_arr[$i]} total=${t} rtp=${r} gb28181=${g} rtpConnection=${c} sipMismatch=${sm} sipRegFail=${srf} sipRollback=${srb} sipOutboundComplete={success:${socs},failed:${socf}} sipClientTimeout=${sct} sipServerTimeout=${sst} sipEnabled=${se} \
rustMain={enabled:${srme},strictEnabled:${srms},rescueEnabled:${srmre},attempt:${sra},success:${srs},fallback:${srfb},strictDrop:${srsd},precheckDrop:${srmpd},postParseFallback:${srmppf},rescueCandidate:${srmrc},rustInvalidFallback:${srmri},rescueParseAttempt:${srmrpa},rescueParseSuccess:${srmrps},rescueParseFailed:${srmrpf},injectRescueFallbackSynthetic:${sirf},injectInvalidFallbackSynthetic:${siif}} \
matchedByMethod={register:${mcrr},message:${mcrm},invite:${mcri},bye:${mcrb},ack:${mcra}} \
clientTimeoutByMethod={register:${sctr},message:${sctm},invite:${scti},bye:${sctb},ack:${scta}} \
serverTimeoutByMethod={register:${sstr},message:${sstm},invite:${ssti},bye:${sstb},ack:${ssta}} warn=${warn_arr[$i]}"
done

declare -a d_total=()
declare -a d_rtp=()
declare -a d_gb=()
declare -a d_conn=()
declare -a d_sip_mismatch=()
declare -a d_sip_reg_fail=()
declare -a d_sip_rollback=()
declare -a d_sip_outbound_complete_success=()
declare -a d_sip_outbound_complete_failed=()
declare -a d_sip_cto=()
declare -a d_sip_sto=()
declare -a d_sip_rmain_attempt=()
declare -a d_sip_rmain_success=()
declare -a d_sip_rmain_fallback=()
declare -a d_sip_rmain_strict_drop=()
declare -a d_sip_rmain_precheck_drop=()
declare -a d_sip_rmain_postparse_fallback=()
declare -a d_sip_rmain_rescue=()
declare -a d_sip_rmain_rust_invalid=()
declare -a d_sip_rmain_rescue_parse_attempt=()
declare -a d_sip_rmain_rescue_parse_success=()
declare -a d_sip_rmain_rescue_parse_failed=()
declare -a d_sip_inject_rescue_fallback=()
declare -a d_sip_inject_invalid_fallback=()
declare -a d_sip_mcr_reg=()
declare -a d_sip_mcr_msg=()
declare -a d_sip_mcr_inv=()
declare -a d_sip_mcr_bye=()
declare -a d_sip_mcr_ack=()
declare -a d_sip_cto_reg=()
declare -a d_sip_cto_msg=()
declare -a d_sip_cto_inv=()
declare -a d_sip_cto_bye=()
declare -a d_sip_cto_ack=()
declare -a d_sip_sto_reg=()
declare -a d_sip_sto_msg=()
declare -a d_sip_sto_inv=()
declare -a d_sip_sto_bye=()
declare -a d_sip_sto_ack=()
declare -a d_warn=()

for i in $(seq 1 "${WINDOWS}"); do
    prev=$((i - 1))
    dt=$(( total_arr[$i] - total_arr[$prev] ))
    dr=$(( rtp_arr[$i] - rtp_arr[$prev] ))
    dg=$(( gb_arr[$i] - gb_arr[$prev] ))
    dc=$(( conn_arr[$i] - conn_arr[$prev] ))
    dsm=$(( sip_mismatch_arr[$i] - sip_mismatch_arr[$prev] ))
    dsrf=$(( sip_reg_fail_arr[$i] - sip_reg_fail_arr[$prev] ))
    dsrb=$(( sip_rollback_arr[$i] - sip_rollback_arr[$prev] ))
    dsocs=$(( sip_outbound_complete_success_arr[$i] - sip_outbound_complete_success_arr[$prev] ))
    dsocf=$(( sip_outbound_complete_failed_arr[$i] - sip_outbound_complete_failed_arr[$prev] ))
    dsct=$(( sip_cto_arr[$i] - sip_cto_arr[$prev] ))
    dsst=$(( sip_sto_arr[$i] - sip_sto_arr[$prev] ))
    dsra=$(( sip_rmain_attempt_arr[$i] - sip_rmain_attempt_arr[$prev] ))
    dsrs=$(( sip_rmain_success_arr[$i] - sip_rmain_success_arr[$prev] ))
    dsrfb=$(( sip_rmain_fallback_arr[$i] - sip_rmain_fallback_arr[$prev] ))
    dsrsd=$(( sip_rmain_strict_drop_arr[$i] - sip_rmain_strict_drop_arr[$prev] ))
    dsrmpd=$(( sip_rmain_precheck_drop_arr[$i] - sip_rmain_precheck_drop_arr[$prev] ))
    dsrmppf=$(( sip_rmain_postparse_fallback_arr[$i] - sip_rmain_postparse_fallback_arr[$prev] ))
    dsrmrc=$(( sip_rmain_rescue_arr[$i] - sip_rmain_rescue_arr[$prev] ))
    dsrmri=$(( sip_rmain_rust_invalid_arr[$i] - sip_rmain_rust_invalid_arr[$prev] ))
    dsrmrpa=$(( sip_rmain_rescue_parse_attempt_arr[$i] - sip_rmain_rescue_parse_attempt_arr[$prev] ))
    dsrmrps=$(( sip_rmain_rescue_parse_success_arr[$i] - sip_rmain_rescue_parse_success_arr[$prev] ))
    dsrmrpf=$(( sip_rmain_rescue_parse_failed_arr[$i] - sip_rmain_rescue_parse_failed_arr[$prev] ))
    dsirf=$(( sip_inject_rescue_fallback_arr[$i] - sip_inject_rescue_fallback_arr[$prev] ))
    dsiif=$(( sip_inject_invalid_fallback_arr[$i] - sip_inject_invalid_fallback_arr[$prev] ))
    dmcrr=$(( sip_mcr_reg_arr[$i] - sip_mcr_reg_arr[$prev] ))
    dmcrm=$(( sip_mcr_msg_arr[$i] - sip_mcr_msg_arr[$prev] ))
    dmcri=$(( sip_mcr_inv_arr[$i] - sip_mcr_inv_arr[$prev] ))
    dmcrb=$(( sip_mcr_bye_arr[$i] - sip_mcr_bye_arr[$prev] ))
    dmcra=$(( sip_mcr_ack_arr[$i] - sip_mcr_ack_arr[$prev] ))
    dsctr=$(( sip_cto_reg_arr[$i] - sip_cto_reg_arr[$prev] ))
    dsctm=$(( sip_cto_msg_arr[$i] - sip_cto_msg_arr[$prev] ))
    dscti=$(( sip_cto_inv_arr[$i] - sip_cto_inv_arr[$prev] ))
    dsctb=$(( sip_cto_bye_arr[$i] - sip_cto_bye_arr[$prev] ))
    dscta=$(( sip_cto_ack_arr[$i] - sip_cto_ack_arr[$prev] ))
    dsstr=$(( sip_sto_reg_arr[$i] - sip_sto_reg_arr[$prev] ))
    dsstm=$(( sip_sto_msg_arr[$i] - sip_sto_msg_arr[$prev] ))
    dssti=$(( sip_sto_inv_arr[$i] - sip_sto_inv_arr[$prev] ))
    dsstb=$(( sip_sto_bye_arr[$i] - sip_sto_bye_arr[$prev] ))
    dssta=$(( sip_sto_ack_arr[$i] - sip_sto_ack_arr[$prev] ))
    if [ "${dt}" -lt 0 ]; then dt=${total_arr[$i]}; fi
    if [ "${dr}" -lt 0 ]; then dr=${rtp_arr[$i]}; fi
    if [ "${dg}" -lt 0 ]; then dg=${gb_arr[$i]}; fi
    if [ "${dc}" -lt 0 ]; then dc=${conn_arr[$i]}; fi
    if [ "${dsm}" -lt 0 ]; then dsm=${sip_mismatch_arr[$i]}; fi
    if [ "${dsrf}" -lt 0 ]; then dsrf=${sip_reg_fail_arr[$i]}; fi
    if [ "${dsrb}" -lt 0 ]; then dsrb=${sip_rollback_arr[$i]}; fi
    if [ "${dsocs}" -lt 0 ]; then dsocs=${sip_outbound_complete_success_arr[$i]}; fi
    if [ "${dsocf}" -lt 0 ]; then dsocf=${sip_outbound_complete_failed_arr[$i]}; fi
    if [ "${dsct}" -lt 0 ]; then dsct=${sip_cto_arr[$i]}; fi
    if [ "${dsst}" -lt 0 ]; then dsst=${sip_sto_arr[$i]}; fi
    if [ "${dsra}" -lt 0 ]; then dsra=${sip_rmain_attempt_arr[$i]}; fi
    if [ "${dsrs}" -lt 0 ]; then dsrs=${sip_rmain_success_arr[$i]}; fi
    if [ "${dsrfb}" -lt 0 ]; then dsrfb=${sip_rmain_fallback_arr[$i]}; fi
    if [ "${dsrsd}" -lt 0 ]; then dsrsd=${sip_rmain_strict_drop_arr[$i]}; fi
    if [ "${dsrmpd}" -lt 0 ]; then dsrmpd=${sip_rmain_precheck_drop_arr[$i]}; fi
    if [ "${dsrmppf}" -lt 0 ]; then dsrmppf=${sip_rmain_postparse_fallback_arr[$i]}; fi
    if [ "${dsrmrc}" -lt 0 ]; then dsrmrc=${sip_rmain_rescue_arr[$i]}; fi
    if [ "${dsrmri}" -lt 0 ]; then dsrmri=${sip_rmain_rust_invalid_arr[$i]}; fi
    if [ "${dsrmrpa}" -lt 0 ]; then dsrmrpa=${sip_rmain_rescue_parse_attempt_arr[$i]}; fi
    if [ "${dsrmrps}" -lt 0 ]; then dsrmrps=${sip_rmain_rescue_parse_success_arr[$i]}; fi
    if [ "${dsrmrpf}" -lt 0 ]; then dsrmrpf=${sip_rmain_rescue_parse_failed_arr[$i]}; fi
    if [ "${dsirf}" -lt 0 ]; then dsirf=${sip_inject_rescue_fallback_arr[$i]}; fi
    if [ "${dsiif}" -lt 0 ]; then dsiif=${sip_inject_invalid_fallback_arr[$i]}; fi
    if [ "${dmcrr}" -lt 0 ]; then dmcrr=${sip_mcr_reg_arr[$i]}; fi
    if [ "${dmcrm}" -lt 0 ]; then dmcrm=${sip_mcr_msg_arr[$i]}; fi
    if [ "${dmcri}" -lt 0 ]; then dmcri=${sip_mcr_inv_arr[$i]}; fi
    if [ "${dmcrb}" -lt 0 ]; then dmcrb=${sip_mcr_bye_arr[$i]}; fi
    if [ "${dmcra}" -lt 0 ]; then dmcra=${sip_mcr_ack_arr[$i]}; fi
    if [ "${dsctr}" -lt 0 ]; then dsctr=${sip_cto_reg_arr[$i]}; fi
    if [ "${dsctm}" -lt 0 ]; then dsctm=${sip_cto_msg_arr[$i]}; fi
    if [ "${dscti}" -lt 0 ]; then dscti=${sip_cto_inv_arr[$i]}; fi
    if [ "${dsctb}" -lt 0 ]; then dsctb=${sip_cto_bye_arr[$i]}; fi
    if [ "${dscta}" -lt 0 ]; then dscta=${sip_cto_ack_arr[$i]}; fi
    if [ "${dsstr}" -lt 0 ]; then dsstr=${sip_sto_reg_arr[$i]}; fi
    if [ "${dsstm}" -lt 0 ]; then dsstm=${sip_sto_msg_arr[$i]}; fi
    if [ "${dssti}" -lt 0 ]; then dssti=${sip_sto_inv_arr[$i]}; fi
    if [ "${dsstb}" -lt 0 ]; then dsstb=${sip_sto_bye_arr[$i]}; fi
    if [ "${dssta}" -lt 0 ]; then dssta=${sip_sto_ack_arr[$i]}; fi
    d_total+=("${dt}")
    d_rtp+=("${dr}")
    d_gb+=("${dg}")
    d_conn+=("${dc}")
    d_sip_mismatch+=("${dsm}")
    d_sip_reg_fail+=("${dsrf}")
    d_sip_rollback+=("${dsrb}")
    d_sip_outbound_complete_success+=("${dsocs}")
    d_sip_outbound_complete_failed+=("${dsocf}")
    d_sip_cto+=("${dsct}")
    d_sip_sto+=("${dsst}")
    d_sip_rmain_attempt+=("${dsra}")
    d_sip_rmain_success+=("${dsrs}")
    d_sip_rmain_fallback+=("${dsrfb}")
    d_sip_rmain_strict_drop+=("${dsrsd}")
    d_sip_rmain_precheck_drop+=("${dsrmpd}")
    d_sip_rmain_postparse_fallback+=("${dsrmppf}")
    d_sip_rmain_rescue+=("${dsrmrc}")
    d_sip_rmain_rust_invalid+=("${dsrmri}")
    d_sip_rmain_rescue_parse_attempt+=("${dsrmrpa}")
    d_sip_rmain_rescue_parse_success+=("${dsrmrps}")
    d_sip_rmain_rescue_parse_failed+=("${dsrmrpf}")
    d_sip_inject_rescue_fallback+=("${dsirf}")
    d_sip_inject_invalid_fallback+=("${dsiif}")
    d_sip_mcr_reg+=("${dmcrr}")
    d_sip_mcr_msg+=("${dmcrm}")
    d_sip_mcr_inv+=("${dmcri}")
    d_sip_mcr_bye+=("${dmcrb}")
    d_sip_mcr_ack+=("${dmcra}")
    d_sip_cto_reg+=("${dsctr}")
    d_sip_cto_msg+=("${dsctm}")
    d_sip_cto_inv+=("${dscti}")
    d_sip_cto_bye+=("${dsctb}")
    d_sip_cto_ack+=("${dscta}")
    d_sip_sto_reg+=("${dsstr}")
    d_sip_sto_msg+=("${dsstm}")
    d_sip_sto_inv+=("${dssti}")
    d_sip_sto_bye+=("${dsstb}")
    d_sip_sto_ack+=("${dssta}")

    if [ "${warn_arr[$i]}" -ge 0 ] && [ "${warn_arr[$prev]}" -ge 0 ]; then
        dw=$(( warn_arr[$i] - warn_arr[$prev] ))
        if [ "${dw}" -lt 0 ]; then dw=${warn_arr[$i]}; fi
    else
        dw=-1
    fi
    d_warn+=("${dw}")

    echo "[gate] window#${i}: delta_total=${dt} delta_rtp=${dr} delta_gb28181=${dg} delta_rtpConnection=${dc} delta_sipMismatch=${dsm} delta_sipRegFail=${dsrf} delta_sipRollback=${dsrb} delta_sipOutboundComplete={success:${dsocs},failed:${dsocf}} delta_sipClientTimeout=${dsct} delta_sipServerTimeout=${dsst} \
delta_rustMain={attempt:${dsra},success:${dsrs},fallback:${dsrfb},strictDrop:${dsrsd},precheckDrop:${dsrmpd},postParseFallback:${dsrmppf},rescueCandidate:${dsrmrc},rustInvalidFallback:${dsrmri},rescueParseAttempt:${dsrmrpa},rescueParseSuccess:${dsrmrps},rescueParseFailed:${dsrmrpf},injectRescueFallbackSynthetic:${dsirf},injectInvalidFallbackSynthetic:${dsiif}} \
delta_sipMatchedByMethod={register:${dmcrr},message:${dmcrm},invite:${dmcri},bye:${dmcrb},ack:${dmcra}} \
delta_sipClientTimeoutByMethod={register:${dsctr},message:${dsctm},invite:${dscti},bye:${dsctb},ack:${dscta}} \
delta_sipServerTimeoutByMethod={register:${dsstr},message:${dsstm},invite:${dssti},bye:${dsstb},ack:${dssta}} delta_warn=${dw}"
done

hold=0
declare -a reasons=()
declare -a advisories=()

# Rule 1: continuous growth across two consecutive windows.
if [ "${WINDOWS}" -ge 2 ]; then
    for i in $(seq 0 $((WINDOWS - 2))); do
        cur=${d_total[$i]}
        next=${d_total[$((i + 1))]}
        cur_window=$((i + 1))
        next_window=$((i + 2))
        if [ "${cur}" -gt 0 ] && [ "${next}" -gt 0 ]; then
            if [ "${ACK_KNOWN_ABNORMAL}" -eq 0 ]; then
                hold=1
                reasons+=("total.decisionMismatch continuous growth: window#${cur_window}=${cur}, window#${next_window}=${next}")
            else
                reasons+=("continuous growth observed but acknowledged as known abnormal traffic")
            fi
        fi
    done
fi

# Rule 2: spike > SPIKE_FACTOR * previous window delta, with warn growth when log file is provided.
if [ "${WINDOWS}" -ge 2 ]; then
    for i in $(seq 0 $((WINDOWS - 2))); do
        prev=${d_total[$i]}
        cur=${d_total[$((i + 1))]}
        base="${prev}"
        cur_window=$((i + 2))
        if [ "${base}" -le 0 ]; then
            base=1
        fi
        threshold=$(( SPIKE_FACTOR * base ))
        if [ "${cur}" -gt "${threshold}" ]; then
            warn_delta=${d_warn[$((i + 1))]}
            if [ -n "${LOG_FILE}" ]; then
                if [ "${warn_delta}" -gt 0 ]; then
                    hold=1
                    reasons+=("spike detected with warn growth: window#${cur_window} delta=${cur}, prev=${prev}, threshold=${threshold}, warnDelta=${warn_delta}")
                fi
            else
                hold=1
                reasons+=("spike detected: window#${cur_window} delta=${cur}, prev=${prev}, threshold=${threshold} (no log-file correlation)")
            fi
        fi
    done
fi

# Rule 3: rustSipShadow registerClientTxFailed must not grow.
for i in $(seq 0 $((WINDOWS - 1))); do
    reg_fail=${d_sip_reg_fail[$i]}
    win=$((i + 1))
    if [ "${reg_fail}" -gt 0 ]; then
        hold=1
        reasons+=("rustSipShadow critical growth: window#${win} registerClientTxFailed=${reg_fail}")
    fi
done

# Rule 3b: rustSipShadow rollbackClientTx indicates send-path failures and should be investigated.
for i in $(seq 0 $((WINDOWS - 1))); do
    rollback=${d_sip_rollback[$i]}
    win=$((i + 1))
    if [ "${rollback}" -gt 0 ]; then
        hold=1
        reasons+=("rustSipShadow critical growth: window#${win} rollbackClientTx=${rollback}")
    fi
done

# Rule 3c: in strict mode, any strict drop or mismatch means behavioral divergence and must HOLD.
for i in $(seq 0 $((WINDOWS - 1))); do
    win=$((i + 1))
    strict_enabled="${sip_rmain_strict_enabled_arr[$((i + 1))]}"
    strict_drop="${d_sip_rmain_strict_drop[$i]}"
    mismatch="${d_sip_mismatch[$i]}"
    if [ "${strict_enabled}" -eq 1 ] && [ "${strict_drop}" -gt 0 ]; then
        hold=1
        reasons+=("rustSipShadow strict mode violation: window#${win} rustMainStrictDrops=${strict_drop}")
    fi
    if [ "${strict_enabled}" -eq 1 ] && [ "${mismatch}" -gt 0 ]; then
        hold=1
        reasons+=("rustSipShadow strict mode mismatch growth: window#${win} mismatchPackets=${mismatch}")
    fi
done

# Rule 3d: rescue parse failures indicate normalization retry failed and must HOLD.
for i in $(seq 0 $((WINDOWS - 1))); do
    rescue_parse_failed="${d_sip_rmain_rescue_parse_failed[$i]}"
    win=$((i + 1))
    if [ "${rescue_parse_failed}" -gt 0 ]; then
        hold=1
        reasons+=("rustSipShadow critical growth: window#${win} rustMainRescueParseFailed=${rescue_parse_failed}")
    fi
done

# Rule 4: rustSipShadow method-level timeout counters must not exceed configured limits.
for i in $(seq 0 $((WINDOWS - 1))); do
    win=$((i + 1))
    cto_reg=${d_sip_cto_reg[$i]}
    cto_msg=${d_sip_cto_msg[$i]}
    cto_inv=${d_sip_cto_inv[$i]}
    cto_bye=${d_sip_cto_bye[$i]}
    cto_ack=${d_sip_cto_ack[$i]}
    sto_reg=${d_sip_sto_reg[$i]}
    sto_msg=${d_sip_sto_msg[$i]}
    sto_inv=${d_sip_sto_inv[$i]}
    sto_bye=${d_sip_sto_bye[$i]}
    sto_ack=${d_sip_sto_ack[$i]}
    traffic_reg=${d_sip_mcr_reg[$i]}
    traffic_msg=${d_sip_mcr_msg[$i]}
    traffic_inv=${d_sip_mcr_inv[$i]}
    traffic_bye=${d_sip_mcr_bye[$i]}
    traffic_ack=${d_sip_mcr_ack[$i]}

    limit_cto_reg="$(compute_method_timeout_limit "${SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER}" "${traffic_reg}")"
    limit_cto_msg="$(compute_method_timeout_limit "${SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE}" "${traffic_msg}")"
    limit_cto_inv="$(compute_method_timeout_limit "${SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE}" "${traffic_inv}")"
    limit_cto_bye="$(compute_method_timeout_limit "${SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE}" "${traffic_bye}")"
    limit_cto_ack="$(compute_method_timeout_limit "${SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK}" "${traffic_ack}")"
    limit_sto_reg="$(compute_method_timeout_limit "${SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER}" "${traffic_reg}")"
    limit_sto_msg="$(compute_method_timeout_limit "${SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE}" "${traffic_msg}")"
    limit_sto_inv="$(compute_method_timeout_limit "${SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE}" "${traffic_inv}")"
    limit_sto_bye="$(compute_method_timeout_limit "${SIP_SERVER_TIMEOUT_MAX_DELTA_BYE}" "${traffic_bye}")"
    limit_sto_ack="$(compute_method_timeout_limit "${SIP_SERVER_TIMEOUT_MAX_DELTA_ACK}" "${traffic_ack}")"

    if [ "${cto_reg}" -gt "${limit_cto_reg}" ] \
        || [ "${cto_msg}" -gt "${limit_cto_msg}" ] \
        || [ "${cto_inv}" -gt "${limit_cto_inv}" ] \
        || [ "${cto_bye}" -gt "${limit_cto_bye}" ] \
        || [ "${cto_ack}" -gt "${limit_cto_ack}" ] \
        || [ "${sto_reg}" -gt "${limit_sto_reg}" ] \
        || [ "${sto_msg}" -gt "${limit_sto_msg}" ] \
        || [ "${sto_inv}" -gt "${limit_sto_inv}" ] \
        || [ "${sto_bye}" -gt "${limit_sto_bye}" ] \
        || [ "${sto_ack}" -gt "${limit_sto_ack}" ]; then
        hold=1
        reasons+=("rustSipShadow method timeout threshold exceeded: window#${win} mode=${SIP_METHOD_TIMEOUT_THRESHOLD_MODE} traffic={register:${traffic_reg},message:${traffic_msg},invite:${traffic_inv},bye:${traffic_bye},ack:${traffic_ack}} client={register:${cto_reg}/${limit_cto_reg},message:${cto_msg}/${limit_cto_msg},invite:${cto_inv}/${limit_cto_inv},bye:${cto_bye}/${limit_cto_bye},ack:${cto_ack}/${limit_cto_ack}} server={register:${sto_reg}/${limit_sto_reg},message:${sto_msg}/${limit_sto_msg},invite:${sto_inv}/${limit_sto_inv},bye:${sto_bye}/${limit_sto_bye},ack:${sto_ack}/${limit_sto_ack}}")
    fi
done

# Rule 5: rustSipShadow mismatchPackets continuous growth across two consecutive windows.
if [ "${WINDOWS}" -ge 2 ]; then
    for i in $(seq 0 $((WINDOWS - 2))); do
        cur=${d_sip_mismatch[$i]}
        next=${d_sip_mismatch[$((i + 1))]}
        cur_window=$((i + 1))
        next_window=$((i + 2))
        if [ "${cur}" -gt 0 ] && [ "${next}" -gt 0 ]; then
            if [ "${ACK_KNOWN_ABNORMAL}" -eq 0 ]; then
                hold=1
                reasons+=("rustSipShadow.mismatchPackets continuous growth: window#${cur_window}=${cur}, window#${next_window}=${next}")
            else
                reasons+=("rustSipShadow mismatch continuous growth observed but acknowledged as known abnormal traffic")
            fi
        fi
    done
fi

# Rule 6: rustSipShadow mismatchPackets spike > SPIKE_FACTOR * previous window delta.
if [ "${WINDOWS}" -ge 2 ]; then
    for i in $(seq 0 $((WINDOWS - 2))); do
        prev=${d_sip_mismatch[$i]}
        cur=${d_sip_mismatch[$((i + 1))]}
        base="${prev}"
        cur_window=$((i + 2))
        if [ "${base}" -le 0 ]; then
            base=1
        fi
        threshold=$(( SPIKE_FACTOR * base ))
        if [ "${cur}" -gt "${threshold}" ]; then
            hold=1
            reasons+=("rustSipShadow.mismatchPackets spike detected: window#${cur_window} delta=${cur}, prev=${prev}, threshold=${threshold}")
        fi
    done
fi

# Rule 7: rust main fallback counters must not exceed optional configured limits.
if [ "${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ]; then
    for i in $(seq 0 $((WINDOWS - 1))); do
        win=$((i + 1))
        precheck_drop=${d_sip_rmain_precheck_drop[$i]}
        postparse_fallback=${d_sip_rmain_postparse_fallback[$i]}
        rescue_candidate=${d_sip_rmain_rescue[$i]}
        rust_invalid_fallback=${d_sip_rmain_rust_invalid[$i]}
        inject_rescue_fallback=${d_sip_inject_rescue_fallback[$i]}
        inject_invalid_fallback=${d_sip_inject_invalid_fallback[$i]}
        if [ "${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA_USER_SET}" -eq 1 ] \
            && [ "${precheck_drop}" -gt "${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA}" ]; then
            hold=1
            reasons+=("rustSipShadow rust main precheck drop threshold exceeded: window#${win} precheckDrop=${precheck_drop}/${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA}")
        fi
        if [ "${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ] \
            && [ "${postparse_fallback}" -gt "${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA}" ]; then
            hold=1
            reasons+=("rustSipShadow rust main post-parse fallback threshold exceeded: window#${win} postParseFallback=${postparse_fallback}/${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA}")
        fi
        if [ "${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA_USER_SET}" -eq 1 ] \
            && [ "${rescue_candidate}" -gt "${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA}" ]; then
            hold=1
            reasons+=("rustSipShadow rust main rescue-candidate threshold exceeded: window#${win} rescueCandidate=${rescue_candidate}/${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA}")
        fi
        if [ "${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ] \
            && [ "${rust_invalid_fallback}" -gt "${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA}" ]; then
            hold=1
            reasons+=("rustSipShadow rust main rust-invalid-fallback threshold exceeded: window#${win} rustInvalidFallback=${rust_invalid_fallback}/${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA}")
        fi
        if [ "${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ] \
            && [ "${inject_rescue_fallback}" -gt "${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA}" ]; then
            hold=1
            reasons+=("rustSipShadow rust main inject rescue-fallback-synthetic threshold exceeded: window#${win} injectRescueFallbackSynthetic=${inject_rescue_fallback}/${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA}")
        fi
        if [ "${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ] \
            && [ "${inject_invalid_fallback}" -gt "${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA}" ]; then
            hold=1
            reasons+=("rustSipShadow rust main inject invalid-fallback-synthetic threshold exceeded: window#${win} injectInvalidFallbackSynthetic=${inject_invalid_fallback}/${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA}")
        fi
    done
fi

# Rule 8 (advisory only): rust main rescue/rust-invalid fallback deltas can emit warnings without HOLD.
if [ "${SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA_USER_SET}" -eq 1 ]; then
    for i in $(seq 0 $((WINDOWS - 1))); do
        win=$((i + 1))
        rescue_candidate=${d_sip_rmain_rescue[$i]}
        rust_invalid_fallback=${d_sip_rmain_rust_invalid[$i]}
        if [ "${SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA_USER_SET}" -eq 1 ] \
            && [ "${rescue_candidate}" -gt "${SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA}" ]; then
            advisories+=("rustSipShadow advisory: rust main rescue-candidate delta high: window#${win} rescueCandidate=${rescue_candidate}/${SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA}")
        fi
        if [ "${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA_USER_SET}" -eq 1 ] \
            && [ "${rust_invalid_fallback}" -gt "${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA}" ]; then
            advisories+=("rustSipShadow advisory: rust main rust-invalid-fallback delta high: window#${win} rustInvalidFallback=${rust_invalid_fallback}/${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA}")
        fi
    done
fi

# Rule 9: outboundCompleteClientTxFailed can optionally trigger HOLD.
if [ "${SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA_USER_SET}" -eq 1 ]; then
    for i in $(seq 0 $((WINDOWS - 1))); do
        win=$((i + 1))
        outbound_complete_failed="${d_sip_outbound_complete_failed[$i]}"
        if [ "${outbound_complete_failed}" -gt "${SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA}" ]; then
            hold=1
            reasons+=("rustSipShadow outbound completion failed threshold exceeded: window#${win} outboundCompleteClientTxFailed=${outbound_complete_failed}/${SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA}")
        fi
    done
fi

# Rule 10 (advisory only): outboundCompleteClientTxFailed growth hint.
for i in $(seq 0 $((WINDOWS - 1))); do
    win=$((i + 1))
    outbound_complete_failed="${d_sip_outbound_complete_failed[$i]}"
    if [ "${SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA_USER_SET}" -eq 1 ]; then
        if [ "${outbound_complete_failed}" -gt "${SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA}" ]; then
            advisories+=("rustSipShadow advisory: outbound completion failed delta high: window#${win} outboundCompleteClientTxFailed=${outbound_complete_failed}/${SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA}")
        fi
    else
        if [ "${outbound_complete_failed}" -gt 0 ]; then
            advisories+=("rustSipShadow advisory: outbound completion failed delta observed: window#${win} outboundCompleteClientTxFailed=${outbound_complete_failed}")
        fi
    fi
done

echo ""
echo "===== Rust Ingress Gate Summary ====="
echo "api_url=${API_URL}"
echo "windows=${WINDOWS}, window_seconds=${WINDOW_SECONDS}, spike_factor=${SPIKE_FACTOR}"
echo "sip_method_timeout_mode=${SIP_METHOD_TIMEOUT_THRESHOLD_MODE} ratio_bps=${SIP_METHOD_TIMEOUT_RATIO_BPS} ratio_min_traffic=${SIP_METHOD_TIMEOUT_RATIO_MIN_TRAFFIC}"
echo "sip_method_timeout_thresholds_absolute: client={register:${SIP_CLIENT_TIMEOUT_MAX_DELTA_REGISTER},message:${SIP_CLIENT_TIMEOUT_MAX_DELTA_MESSAGE},invite:${SIP_CLIENT_TIMEOUT_MAX_DELTA_INVITE},bye:${SIP_CLIENT_TIMEOUT_MAX_DELTA_BYE},ack:${SIP_CLIENT_TIMEOUT_MAX_DELTA_ACK}} server={register:${SIP_SERVER_TIMEOUT_MAX_DELTA_REGISTER},message:${SIP_SERVER_TIMEOUT_MAX_DELTA_MESSAGE},invite:${SIP_SERVER_TIMEOUT_MAX_DELTA_INVITE},bye:${SIP_SERVER_TIMEOUT_MAX_DELTA_BYE},ack:${SIP_SERVER_TIMEOUT_MAX_DELTA_ACK}}"
if [ "${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA_USER_SET}" -eq 1 ]; then
    echo "sip_rust_main_fallback_thresholds: precheckDrop=${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA}(enabled=${SIP_RUST_MAIN_PRECHECK_DROP_MAX_DELTA_USER_SET}) postParseFallback=${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA}(enabled=${SIP_RUST_MAIN_POSTPARSE_FALLBACK_MAX_DELTA_USER_SET}) rescueCandidate=${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA}(enabled=${SIP_RUST_MAIN_RESCUE_CANDIDATE_MAX_DELTA_USER_SET}) rustInvalidFallback=${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA}(enabled=${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_MAX_DELTA_USER_SET}) injectRescueFallbackSynthetic=${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA}(enabled=${SIP_RUST_MAIN_INJECT_RESCUE_FALLBACK_MAX_DELTA_USER_SET}) injectInvalidFallbackSynthetic=${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA}(enabled=${SIP_RUST_MAIN_INJECT_INVALID_FALLBACK_MAX_DELTA_USER_SET})"
else
    echo "sip_rust_main_fallback_thresholds: disabled"
fi
if [ "${SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA_USER_SET}" -eq 1 ] \
    || [ "${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA_USER_SET}" -eq 1 ]; then
    echo "sip_rust_main_advisory_thresholds: rescueCandidateWarn=${SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA}(enabled=${SIP_RUST_MAIN_RESCUE_CANDIDATE_WARN_DELTA_USER_SET}) rustInvalidFallbackWarn=${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA}(enabled=${SIP_RUST_MAIN_RUST_INVALID_FALLBACK_WARN_DELTA_USER_SET})"
else
    echo "sip_rust_main_advisory_thresholds: disabled"
fi
if [ "${SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA_USER_SET}" -eq 1 ]; then
    echo "sip_outbound_completion_thresholds: outboundCompleteClientTxFailedMax=${SIP_OUTBOUND_COMPLETE_FAILED_MAX_DELTA}(enabled=1)"
else
    echo "sip_outbound_completion_thresholds: outboundCompleteClientTxFailedMax=disabled"
fi
if [ "${SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA_USER_SET}" -eq 1 ]; then
    echo "sip_outbound_completion_advisory_thresholds: outboundCompleteClientTxFailedWarn=${SIP_OUTBOUND_COMPLETE_FAILED_WARN_DELTA}(enabled=1)"
else
    echo "sip_outbound_completion_advisory_thresholds: outboundCompleteClientTxFailedWarn=default(any growth > 0)"
fi
if [ "${sip_enabled_arr[0]}" = "1" ]; then
    echo "rust_sip_shadow=enabled"
else
    echo "rust_sip_shadow=disabled_or_missing"
fi
if [ -n "${LOG_FILE}" ]; then
    echo "log_file=${LOG_FILE}"
else
    echo "log_file=<none>"
fi

if [ "${#reasons[@]}" -gt 0 ]; then
    for reason in "${reasons[@]}"; do
        echo "reason: ${reason}"
    done
else
    echo "reason: no gate violations"
fi
if [ "${#advisories[@]}" -gt 0 ]; then
    for advisory in "${advisories[@]}"; do
        echo "advisory: ${advisory}"
    done
fi

if [ "${hold}" -eq 1 ]; then
    echo "result=HOLD"
    exit 10
fi

echo "result=PASS"
exit 0
