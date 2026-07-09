# cheetah-rtsp-fuzz

RTSP 协议相关的独立 fuzz 目标集合。

## Build

```bash
cd crates/protocols/rtsp/fuzz
cargo +nightly fuzz build
```

## Run

```bash
cd crates/protocols/rtsp/fuzz
cargo +nightly fuzz run fuzz_rtsp_core
```

## Corpus Conventions

- `corpus/<target>/seed_standard_*_prefix.rtspcap`: 标准互操作路径前缀（publish/play/pull/push）。
- `corpus/<target>/seed_probe_*_prefix.rtspcap`: 兼容探测路径前缀（AV1/VP9/PS/异常 SDP 可恢复分支）。
- `corpus/<target>/seed_fault_*_prefix.rtspcap`: 传输或控制面故障前缀（bad SDP、multicast/http tunnel fault）。
- 所有 prefix 使用 `head -c 65536` 从 `testing/property-tests/tests/testdata/rtsp-capture/**/*.rtspcap` 裁剪，保证语义代表性和 fuzz 吞吐。
