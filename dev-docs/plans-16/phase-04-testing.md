# Phase 04 — 测试

- **状态**: 未开始
- **范围**: Fuzz harness、property-based 测试、集成测试、端到端验证
- **完成标准**: fuzz 运行 10 分钟无 panic；property tests 覆盖核心不变量；CI 端到端通过

---

## 4.1 TS Muxer Fuzz Harness

**目标**: 确保 `TsMuxer` 对任意输入不 panic、不产生非 188 字节对齐的输出。

**目录**: `crates/protocols/hls/fuzz/` (独立 cargo-fuzz workspace)

**Fuzz 目标**:

```rust
// fuzz_targets/ts_muxer.rs
fuzz_target!(|data: &[u8]| {
    if data.len() < 10 { return; }
    let mut muxer = TsMuxer::new(CodecId::H264, true);
    muxer.write_pat_pmt();

    // 随机切分 data 为多帧
    let pts = u64::from_le_bytes(data[0..8].try_into().unwrap_or([0;8]));
    let is_key = data[8] & 1 == 1;
    muxer.write_video(&data[9..], pts, pts, is_key);

    let segment = muxer.take_segment();
    assert_eq!(segment.len() % 188, 0);
    // 每个包首字节必须是 0x47
    for chunk in segment.chunks(188) {
        assert_eq!(chunk[0], 0x47);
    }
});
```

**不变量**:
- 输出长度 % 188 == 0
- 每 188 字节块首字节 == 0x47 (sync byte)
- continuity counter 在同一 PID 上单调递增 (mod 16)

---

## 4.2 M3U8 Playlist Property-Based 测试

**目标**: 验证 `PlaylistBuilder` 输出满足 HLS 规范不变量。

**目录**: `crates/protocols/hls/testing/property-tests/`

**Property 列表**:

```rust
#[test]
fn prop_media_playlist_invariants(segments in vec(segment_strategy(), 1..20)) {
    let mut ring = SegmentRing::new(10);
    for seg in &segments {
        ring.push(seg.name.clone(), seg.duration, seg.data.clone(), true);
    }
    let m3u8 = PlaylistBuilder::build_media(&ring, None);

    // P1: 必须以 #EXTM3U 开头
    assert!(m3u8.starts_with("#EXTM3U\n"));
    // P2: TARGETDURATION >= 所有 EXTINF duration 的 ceil
    let td = parse_target_duration(&m3u8);
    for d in parse_extinf_durations(&m3u8) {
        assert!(td >= d.ceil() as u64);
    }
    // P3: segment 数量 == ring 中 segment 数量
    let seg_count = m3u8.matches("#EXTINF:").count();
    assert_eq!(seg_count, ring.len());
    // P4: 无 EXT-X-ENDLIST（live）
    assert!(!m3u8.contains("#EXT-X-ENDLIST"));
}
```

---

## 4.3 请求路由 Fuzz

**目标**: 确保 `parse_hls_request` 对任意 URL 不 panic。

```rust
// fuzz_targets/request_parser.rs
fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse_hls_request(s);
    }
});
```

**不变量**:
- 不 panic
- 返回 Ok 时 stream_key 字段非空
- 返回 Err 时错误类型正确

---

## 4.4 端到端集成测试

**目标**: 验证完整推流→HLS 播放链路。

**测试脚本**: `dev-scripts/check_hls_smoke.sh`

```bash
#!/bin/bash
set -e

# 启动服务器
cargo build -p cheetah-server --features hls
./target/debug/cheetah-server &
SERVER_PID=$!
sleep 2

# 推流
ffmpeg -re -f lavfi -i testsrc=duration=10:size=320x240:rate=25 \
       -f lavfi -i sine=frequency=440:duration=10 \
       -c:v libx264 -c:a aac -f flv rtmp://127.0.0.1:1935/live/test &
FFMPEG_PID=$!
sleep 6

# 拉取 M3U8
HTTP_CODE=$(curl -s -o /tmp/hls_test.m3u8 -w "%{http_code}" http://127.0.0.1:8088/live/test.m3u8)
[ "$HTTP_CODE" = "200" ] || { echo "FAIL: master playlist HTTP $HTTP_CODE"; exit 1; }
grep -q "#EXTM3U" /tmp/hls_test.m3u8 || { echo "FAIL: not valid m3u8"; exit 1; }

# 拉取 media playlist
MEDIA_URL=$(grep -v "^#" /tmp/hls_test.m3u8 | head -1)
HTTP_CODE=$(curl -s -o /tmp/hls_media.m3u8 -w "%{http_code}" "http://127.0.0.1:8088/live/$MEDIA_URL")
[ "$HTTP_CODE" = "200" ] || { echo "FAIL: media playlist HTTP $HTTP_CODE"; exit 1; }
grep -q "#EXTINF:" /tmp/hls_media.m3u8 || { echo "FAIL: no segments in media playlist"; exit 1; }

# 拉取第一个 segment
SEG_URL=$(grep -v "^#" /tmp/hls_media.m3u8 | head -1)
HTTP_CODE=$(curl -s -o /tmp/hls_seg.ts -w "%{http_code}" "http://127.0.0.1:8088/live/test/$SEG_URL")
[ "$HTTP_CODE" = "200" ] || { echo "FAIL: segment HTTP $HTTP_CODE"; exit 1; }
[ -s /tmp/hls_seg.ts ] || { echo "FAIL: segment is empty"; exit 1; }

# 验证 TS 文件有效
ffprobe -v error /tmp/hls_seg.ts || { echo "FAIL: invalid TS segment"; exit 1; }

echo "PASS: HLS smoke test"
kill $FFMPEG_PID $SERVER_PID 2>/dev/null || true
```

---

## 4.5 单元测试补充清单

| 模块 | 测试 | 覆盖点 |
|------|------|--------|
| `ts_mux` | `test_all_codec_stream_types` | 每种 CodecId 映射正确 |
| `ts_mux` | `test_aud_injection_h264` | H264 PES 以 AUD 开头 |
| `ts_mux` | `test_aud_injection_h265` | H265 PES 以 AUD 开头 |
| `ts_mux` | `test_audio_adts_wrap` | AAC PES 包含 ADTS 头 |
| `ts_mux` | `test_cc_wraps_at_16` | CC 在 15→0 正确回绕 |
| `ts_mux` | `test_large_frame_multi_packet` | 大帧正确分包 |
| `playlist` | `test_fmp4_playlist_has_map` | fMP4 模式包含 EXT-X-MAP |
| `playlist` | `test_target_duration_ceil` | TARGETDURATION ≥ max(EXTINF) |
| `segment` | `test_ring_sequence_monotonic` | sequence 单调递增 |
| `request` | `test_deep_path_rejected` | 4+ 层路径返回错误 |
| `session` | `test_head_method_same_as_get` | HEAD 和 GET 产生相同事件 |

---

## Crate 组织

```
crates/protocols/hls/
├── core/                          (cheetah-hls-core)
├── driver-tokio/                  (cheetah-hls-driver-tokio)
├── module/                        (cheetah-hls-module)
├── testing/
│   └── property-tests/            (cheetah-hls-property-tests)
└── fuzz/                          (独立 cargo-fuzz workspace)
    ├── Cargo.toml
    └── fuzz_targets/
        ├── ts_muxer.rs
        └── request_parser.rs
```
