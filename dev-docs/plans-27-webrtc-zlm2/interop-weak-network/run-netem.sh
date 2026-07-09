#!/usr/bin/env bash
# Phase 06 — Linux `tc netem` wrapper used by the weak-network
# interop suite. Wraps a single command in a netem profile applied
# to the loopback interface so the cheetah driver and any
# co-located helper (Pion, GStreamer, etc.) experience the
# configured impairment.
#
# Usage:
#   sudo dev-docs/plans-27-webrtc-zlm2/interop-weak-network/run-netem.sh \
#       <profile> -- <command>
#
# Profiles (matching the `min_nacks_under_loss` thresholds in
# `module/tests/interop_harness.rs::assertions::InteropThresholds`):
#   loss-1   1% random loss
#   loss-5   5% random loss
#   loss-10  10% random loss
#   loss-20  20% random loss
#   reorder  delay 30 ms reorder 25%
#   bw-cap   rate 1 Mbit
#
# Example:
#   sudo run-netem.sh loss-10 -- \
#       cargo test -p cheetah-webrtc-module --test interop -- --ignored \
#         weak_network_nack_recovery
#
# Note: requires root for `tc qdisc`. On Windows / macOS use a
# WireMock-style network simulator instead; the harness skips weak
# network tests when WEBRTC_INTEROP_WEAK_NETWORK is unset.

set -euo pipefail

if [[ ${EUID} -ne 0 ]]; then
  echo "must run as root: sudo $0 <profile> -- <command...>" >&2
  exit 1
fi

if [[ $# -lt 2 ]]; then
  echo "usage: $0 <profile> -- <command...>" >&2
  exit 1
fi

profile=$1
shift
if [[ "$1" != "--" ]]; then
  echo "expected '--' separator, got $1" >&2
  exit 1
fi
shift

case "$profile" in
  loss-1)   netem_args=(loss 1%) ;;
  loss-5)   netem_args=(loss 5%) ;;
  loss-10)  netem_args=(loss 10%) ;;
  loss-20)  netem_args=(loss 20%) ;;
  reorder)  netem_args=(delay 30ms reorder 25%) ;;
  bw-cap)   netem_args=(rate 1mbit) ;;
  *)
    echo "unknown profile: $profile" >&2
    exit 1
    ;;
esac

cleanup() {
  tc qdisc del dev lo root 2>/dev/null || true
}
trap cleanup EXIT INT TERM

tc qdisc add dev lo root netem "${netem_args[@]}"
echo "[netem] applied profile=$profile args=${netem_args[*]} on dev lo" >&2

# Forward signal to child so cleanup can run.
"$@"
