# Weak network on Windows / macOS

Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`).

`tc netem` is Linux-only. On Windows and macOS we use third-party
network simulators that aren't available as a single CLI flag. The
cheetah harness (`module/tests/interop.rs::weak_network_nack_recovery`)
respects `WEBRTC_INTEROP_WEAK_NETWORK=1` and skips automatically
otherwise, so cross-platform CI runs stay green by default.

## Windows: Clumsy

[Clumsy](https://jagt.github.io/clumsy/) is a small WinDivert-based
GUI/CLI that injects loss / reorder / drop / lag rules. Run it as
Administrator before launching the harness.

Recommended CLI invocation (Clumsy 0.3+):

```powershell
# 10% random loss on the loopback interface
.\clumsy.exe `
  --filter "loopback" `
  --drop on --drop-chance 10.0 `
  --start

# Run the cheetah harness in a separate terminal
$env:WEBRTC_INTEROP_WEAK_NETWORK="1"
cargo test -p cheetah-webrtc-module --test interop -- --ignored weak_network_nack_recovery

# Stop Clumsy when the test finishes
.\clumsy.exe --stop
```

Mapping `tc netem` profiles to Clumsy options:

| Linux profile  | Clumsy options                                |
|----------------|-----------------------------------------------|
| `loss-1`       | `--drop on --drop-chance 1.0`                 |
| `loss-5`       | `--drop on --drop-chance 5.0`                 |
| `loss-10`      | `--drop on --drop-chance 10.0`                |
| `loss-20`      | `--drop on --drop-chance 20.0`                |
| `reorder`      | `--out-of-order on --out-of-order-chance 25.0` |
| `bw-cap`       | `--bandwidth on --bandwidth-bandwidth 1024`   |

Save these as PowerShell launcher scripts (e.g.
`run-clumsy-loss10.ps1`) so iteration is quick.

## Windows: pktmon (Windows 10 1809+)

`pktmon` is the built-in Windows packet monitor. It can capture and
filter but cannot inject impairment, so it's only useful for *
observation* — pair it with Clumsy when you need both.

```powershell
pktmon filter add cheetah --port 8000
pktmon start --capture --comp 0
# ... run the test ...
pktmon stop
pktmon etl2pcap PktMon.etl --out cheetah.pcap
```

Drop the resulting pcap into the artifact dir
(`$env:WEBRTC_INTEROP_ARTIFACT_DIR/cheetah.pcap`) for offline
analysis.

## macOS: Network Link Conditioner

Apple ships [Network Link Conditioner](https://developer.apple.com/download/all/?q=Additional%20Tools)
inside the Additional Tools for Xcode. It exposes preset profiles
(`100% Loss`, `Edge`, `LTE`, etc.) plus a custom dialog. There is
no built-in CLI; the harness skips on macOS unless the operator
runs the suite manually with the conditioner enabled.

Recommended conditioner profiles for cheetah weak-network runs:

- "Very Bad Network" → matches `loss-10`
- "3G" → matches `bw-cap` (constrained bandwidth)
- "Edge" → matches `bw-cap` plus added latency

## CI considerations

- Linux nightly already has the `weak-network` job
  (`tc netem` matrix; manual dispatch).
- Windows + macOS CI typically runs on hosted runners without the
  network simulators above, so the harness skip is the right
  behaviour. Operators run the suite locally before merging
  changes that affect RTX / NACK / BWE paths.
- If a self-hosted Windows runner with Clumsy is available, you
  can extend `webrtc-interop-nightly.yml` with a
  `runs-on: [self-hosted, windows]` job mirroring the Linux
  matrix. Keep the matrix off the default schedule until the
  runner is confirmed stable.

## Status

This document is the minimum viable cross-platform contract: the
harness skips when impairment isn't applied, and operators have a
copy-paste recipe for each platform. Full automation on Windows
needs a self-hosted runner with Clumsy preinstalled.
