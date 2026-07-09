# cheetah-rtmp-fuzz

Standalone fuzz targets for RTMP core/module wire handling.

## Build

```bash
cd crates/protocols/rtmp/fuzz
cargo +nightly fuzz build
```

## CI Smoke Check

Compile-only smoke (default):

```bash
./dev-scripts/check_rtmp_fuzz_smoke.sh
```

Runtime smoke (bounded run per target):

```bash
cargo +nightly install cargo-fuzz --locked
RUN_FUZZ_SMOKE_RUN=1 FUZZ_MAX_TOTAL_TIME=3 ./dev-scripts/check_rtmp_fuzz_smoke.sh
```

Run a subset:

```bash
RUN_FUZZ_SMOKE_RUN=1 FUZZ_TARGETS="fuzz_amf0 fuzz_rtmp_chunk" ./dev-scripts/check_rtmp_fuzz_smoke.sh
```

## Run

```bash
cd crates/protocols/rtmp/fuzz
cargo +nightly fuzz run fuzz_rtmp_chunk
```
