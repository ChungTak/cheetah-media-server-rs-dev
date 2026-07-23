# GB28181/RTP Media Runbook

This runbook covers common operational scenarios for the `cheetah-rtp-module`
media data plane. It assumes the engine is assembled with the RTP module enabled
and that external signaling (SIP/SDP/MANSCDP/XML) is handled by a third-party
system that calls the typed media API.

All diagnostic commands below target the engine metrics/text endpoint and the
RTP module HTTP routes. Adjust host/port to match your deployment.

- Metrics endpoint: `http://localhost:8080/metrics`
- RTP module API prefix: `/api/v1/rtp`

---

## 1. Port exhaustion

### Symptom

`open_receiver` or `open_sender` returns `Unavailable` or the HTTP endpoint
returns `code != 200` with a message mentioning port allocation. New sessions
cannot be created even though `rtp_sessions_active` is below `max_sessions`.

### Diagnostic signals

- Metric `rtp_port_pool_exhausted_total` (if exposed by the driver) or a spike
  in `rtp_sessions_failed_total`.
- Log line: `port pool exhausted` or `bind() failed` from `cheetah-rtp-driver-tokio`.

### Checks

```bash
# Active sessions and configured limits
curl -s http://localhost:8080/metrics | grep -E 'rtp_sessions_active|rtp_port_pool|rtp_sessions_failed_total'

# Operating-system ephemeral port usage (Linux)
ss -s
```

### Remediation

1. Verify the configured pool is not empty and does not overlap with well-known ports:
   ```yaml
   modules:
     rtp:
       udp_port_pool_start: 30000
       udp_port_pool_end: 40000
   ```
2. Confirm stale sessions are stopped. Leaked sessions can hold ports even
   when inactive:
   ```bash
   curl -s -X POST http://localhost:8080/api/v1/rtp/reconcile \
     -H 'Content-Type: application/json' \
     -d '{"keep": []}'
   ```
3. If the pool is legitimately too small, extend the range and restart the RTP
   module (the change requires `ModuleRestartRequired`).
4. Check for OS-level socket leaks (`ss -s` high `tcp:` or `udp:` counts) and
   tune `net.ipv4.ip_local_port_range` if needed.

---

## 2. RTP no stream (receiver does not go online)

### Symptom

`/api/v1/rtp/server/create` returns `200` with a port, but no `media_online`
event is emitted and no frames are published to the engine.

### Diagnostic signals

- `rtp_sessions_active` increases but `media_online` events do not appear.
- Log line: `RTP acquire_publisher failed for ...` from the ingress worker.
- Metric `rtp_sessions_failed_total` may increase if the publisher lease is denied.

### Checks

```bash
# Verify the receiver session state and remote endpoint
curl -s http://localhost:8080/api/v1/rtp/sessions | jq '.items[] | select(.kind == "RtpReceiver")'

# Check whether another stream already holds the publish lease
curl -s http://localhost:8080/api/v1/streams | jq '.[] | select(.key.namespace == "live" and .key.path == "<stream>")'
```

### Remediation

1. Confirm the device is sending to the reported `port` and `local` address.
2. Check for publisher conflicts. A `StreamKey` can have only one active publisher
   at a time. Stop the conflicting session first.
3. If no UDP packets arrive, verify firewalls/NAT and that the device is using
   the correct SSRC/payload type.
4. Increase logging around `RtpCoreEvent::Frame` to verify packets are being
   received and demuxed.
5. If `acquire_publisher` fails because the upstream module is not ready, verify
   the target stream module (e.g. HLS/MP4) is running.

---

## 3. PS stream has no PSM (Program Stream Map)

### Symptom

A PS-over-RTP receiver opens but `TrackFound` never arrives; the demuxer stays
in the probing state.

### Diagnostic signals

- Log line: `PS demuxer waiting for PSM` or repeated `probe packet` messages.
- Metric `cheetah_codec_ps_probe_packets` (or `rtp_sessions_failed_total` after
  the probe limit is hit).

### Checks

```bash
# Session snapshot
curl -s http://localhost:8080/api/v1/rtp/sessions/<session_id> | jq .

# Recent demuxer diagnostics (if dumped to logs)
grep -i 'psm\|probe' /var/log/cheetah.log | tail -50
```

### Remediation

1. Confirm the encoder is sending a valid PS with a PSM. Some devices send raw
   PES without PSM after a long gap; this is `Unsupported` by design for strict
   profile and `Experimental` for `gb28181_common`.
2. Switch to `gb28181_common` or `zlm` profile if the device omits PSM but the
   PT/static binding is sufficient.
3. Check `max_probe_bytes` / `max_probe_packets` limits are not too low for
   high-latency links.
4. Capture a short RTP fixture and replay it through the unit tests in
   `crates/foundation/cheetah-codec/src/ps/tests.rs` to verify the parser.

---

## 4. SSRC conflict / spoofed source

### Symptom

An existing receiver stops receiving or switches to a different source mid-stream.
`RtpCore` emits `SourceChanged` or drops packets with `source rejected`.

### Diagnostic signals

- Log: `RTP source address rebind: ...` or `source rejected`.
- Metric `rtp_session_source_rejected_total` (if exposed) or a spike in
  `rtp_sessions_failed_total`.

### Checks

```bash
# Session source binding policy
curl -s http://localhost:8080/api/v1/rtp/sessions/<session_id> | jq '.source_binding_policy, .remote_endpoint'

# Recent source events
grep -i 'source' /var/log/cheetah.log | tail -20
```

### Remediation

1. For trusted networks, ensure the device is not reusing SSRCs across sessions.
2. Use `Strict` source binding for public/untrusted networks and pre-negotiated
   SSRCs.
3. If legitimate rebind is expected (mobile/NAT), use `AllowValidatedRebind`
   with `max_rebind_attempts` and rebind rate limits.
4. If a spoofing attack is suspected, rotate ports and report the incident; the
   module rejects the source but the network should be filtered at the firewall.

---

## 5. Unexpected source change

### Symptom

The remote endpoint for a receiver changes without an explicit update call.

### Diagnostic signals

- `RtpCoreEvent::SourceChanged` in logs.
- `rtp_source_address_rebind` events.

### Checks

```bash
# Current remote endpoint
curl -s http://localhost:8080/api/v1/rtp/sessions/<session_id> | jq '.endpoints.remote'
```

### Remediation

1. Review the source binding policy. `Strict` binds to the first source address/SSRC
   and rejects changes. `AllowValidatedRebind` permits a single validated change.
2. Ensure `source_binding_policy` is passed on `open_receiver` and not omitted.
3. If NAT rebinding is expected, tune the idle/rebind window rather than disabling
   source validation.

---

## 6. RTCP timeout

### Symptom

A sender session is closed with reason `RrTimeout` or `IdleTimeout` even though
RTP packets are being transmitted.

### Diagnostic signals

- `SessionClosed` event with `reason: Timeout`.
- Metric `rtp_sessions_closed_total` increases while `rtp_sessions_active` drops.

### Checks

```bash
# RTCP configuration
curl -s http://localhost:8080/metrics | grep -E 'rtcp_report_interval|session_idle_timeout'

# Session close events
grep 'RTP ingress session closed' /var/log/cheetah.log | tail -20
```

### Remediation

1. Verify the peer is sending RTCP receiver reports (RR) and that the network
   path allows RTCP traffic.
2. For unidirectional/firewalled receivers, configure the peer to send RR or
   disable RTCP-based timeout in the RTP module config (`rtcp_report_interval_ms`,
   `idle_timeout_ms`).
3. If RTCP is muxed with RTP on a single port, ensure `rtcp_listen_udp` is not
   configured to a separate port that never receives traffic.
4. For pull/sender sessions, confirm the remote endpoint is reachable and sending
   any traffic at all; the idle timeout closes dead sessions to free ports.

---

## 7. Controller / registry outage

### Symptom

Media API calls return `Unavailable` or `Conflict`. The module cannot register
`RtpSessionApi` during startup, or `MediaServices` reports no provider.

### Diagnostic signals

- Log: `media services registration failed` or `capability unavailable`.
- Engine health endpoint returns non-200.

### Checks

```bash
# Engine health
curl -s http://localhost:8080/health

# Registered providers
curl -s http://localhost:8080/api/v1/capabilities | jq .
```

### Remediation

1. Restart the module via the engine lifecycle (`ModuleRestartRequired`) rather
   than restarting the whole process.
2. If the outage is the control plane/registry, the media data plane may continue
   to run; avoid draining until the control plane is restored.
3. Verify `MediaServices` has an `RtpSessionApi` provider registered after restart.
4. Check that `cheetah-rtp-module` feature is compiled and enabled in config.

---

## 8. Rollback incomplete

### Symptom

A session creation fails (`open_*` returns an error) but a port, task, or lease
remains allocated. `rtp_rollback_total` is greater than zero and resources do
not converge.

### Diagnostic signals

- `RtpModuleMetrics` shows `rollback_total > 0` and `sessions_active` does not
  return to the pre-request value.
- Leaked ports visible in `ss -uln` that are not listed in active sessions.

### Checks

```bash
# Rollback and active counters
curl -s http://localhost:8080/metrics | grep -E 'rtp_rollback_total|rtp_sessions_active|rtp_sessions_failed_total'

# Active sessions vs OS sockets
curl -s http://localhost:8080/api/v1/rtp/sessions | jq '.items | length'
```

### Remediation

1. Use `reconcile_sessions` (or the `/api/v1/rtp/reconcile` adapter) to stop any
   sessions not in the expected keep list:
   ```bash
   curl -s -X POST http://localhost:8080/api/v1/rtp/reconcile \
     -H 'Content-Type: application/json' \
     -d '{"keep": ["session-id-1"]}'
   ```
2. If a rollback repeatedly fails, inspect the driver event loop for panics or
   hung `send_command` calls.
3. Restart the RTP module as a last resort. The `RtpModule::stop()` path cancels
   all in-flight tasks and clears the egress/client target maps.
4. After restart, compare `rtp_sessions_active` with the orchestrator session
   count; any mismatch indicates a leak and should be filed as a bug.

---

## Metrics quick reference

| Metric | Meaning |
| --- | --- |
| `rtp_sessions_active` | Current orchestrator session count |
| `rtp_sessions_requested_total` | Total `open_*` requests |
| `rtp_sessions_opened_total` | Successful session opens |
| `rtp_sessions_failed_total` | Failed opens/updates |
| `rtp_sessions_closed_total` | Successful stops |
| `rtp_rollback_total` | Rollback guard cleanups |
| `rtp_sessions_rate_limited_total` | Per-principal rate-limit hits |
| `rtp_sessions_admission_denied_total` | Admission denials |

## See also

- `SystemArchitecture.md` — RTP/core/driver/module split and Sans-I/O boundary.
- `AGENTS.md` — engineering constraints (module naming, no SIP/SDP/XML in Cheetah).
- `dev-docs/plans-29-gb28181-impove/10_security_observability_operations.md`
- `dev-docs/plans-29-gb28181-impove/13_release_evidence_template.md`
