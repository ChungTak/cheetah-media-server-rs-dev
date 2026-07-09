# SRT Fuzz Targets

This workspace is intentionally independent and is not included in the root
Cargo workspace.

Run targets with `cargo fuzz` from this directory:

```bash
cargo fuzz run fuzz_stream_id
cargo fuzz run fuzz_srt_url
cargo fuzz run fuzz_driver_packet
```

Targets:

- `fuzz_stream_id`: feeds arbitrary bytes into the SRT Stream ID parser.
- `fuzz_srt_url`: feeds arbitrary bytes into the SRT URL parser.
- `fuzz_driver_packet`: feeds arbitrary UDP payload bytes into the
  `shiguredo_srt` listener-side Sans-I/O packet path.

