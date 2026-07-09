# cheetah-webrtc-fuzz

`cargo-fuzz` harnesses for the WebRTC core. This crate is **not** part
of the root workspace because `libfuzzer-sys` requires nightly Rust;
`Cargo.toml` declares an empty `[workspace]` table to keep its build
isolated.

## Run

```sh
cd crates/protocols/webrtc/fuzz
cargo +nightly fuzz run fuzz_sdp_compat
cargo +nightly fuzz run fuzz_zlm_rtc_url
cargo +nightly fuzz run fuzz_tcp_framing
cargo +nightly fuzz run fuzz_trickle_candidates
```

## Targets

| Target | Asserted invariants |
|--------|--------------------|
| `fuzz_sdp_compat` | preprocessor is idempotent, never panics, non-empty output ends with `\r\n` |
| `fuzz_zlm_rtc_url` | parser never panics; on success `host`, `app`, `stream` are non-empty |
| `fuzz_tcp_framing` | RFC 4571 decoder never panics, drains correctly when bytes arrive one-at-a-time |
| `fuzz_trickle_candidates` | extractor never panics; every extracted line starts with `candidate:` and is longer than the prefix |
