# Weak network interop runner

Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`).

The cheetah weak-network suite uses Linux `tc netem` to inject
loss / reorder / bandwidth caps on the loopback interface. The
harness (`module/tests/interop.rs::weak_network_nack_recovery`) is
gated by the `WEBRTC_INTEROP_WEAK_NETWORK` env var so default CI
runs skip it.

## Linux

```bash
sudo dev-docs/plans-27-webrtc-zlm2/interop-weak-network/run-netem.sh \
    loss-10 -- \
    env WEBRTC_INTEROP_WEAK_NETWORK=1 \
    cargo test -p cheetah-webrtc-module --test interop \
    -- --ignored weak_network_nack_recovery
```

The script applies the named profile to `dev lo`, forwards the
command, and cleans up the qdisc on exit (success or failure).

## macOS / Windows

`tc netem` is Linux-only. Use a 3rd-party network simulator
(Clumsy, Network Link Conditioner, Toxiproxy) and skip the script.
The harness still skips automatically when the env is unset.

## Profiles

| Profile  | tc args              | Use case                          |
|----------|----------------------|-----------------------------------|
| loss-1   | loss 1%              | sanity baseline                   |
| loss-5   | loss 5%              | NACK / RTX engages                |
| loss-10  | loss 10%             | recovery threshold                |
| loss-20  | loss 20%             | severe; expect BWE drop           |
| reorder  | delay 30ms reorder 25% | jitter buffer reorder           |
| bw-cap   | rate 1mbit           | BWE drop and recovery             |

When adding a new profile, also extend the `match` in
`run-netem.sh` and document the expected NACK / BWE thresholds in
`module/tests/interop_harness.rs::assertions::InteropThresholds`
so the assertion helpers can validate the run.
