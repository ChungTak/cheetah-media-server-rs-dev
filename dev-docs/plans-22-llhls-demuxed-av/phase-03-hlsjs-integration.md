# Phase 03 — hls.js 集成验证与端到端测试

- **目标**: 验证 demuxed audio/video LLHLS 在 hls.js + Chrome 中创建独立 SourceBuffer 并正常播放，同时保证 ffplay、TS HLS、video-only/audio-only 回归场景可用
- **OME 参考**: OvenPlayer/hls.js 集成行为、`llhls_chunklist.cpp::MakeChunklist` 的 rendition report 与 preload hint

---

## 1. 内嵌播放页

hls.js 能自动处理 demuxed audio rendition，但播放页不能只做“能加载”的烟测。内嵌页需要保留 LLHLS 配置，并增加可观测信息：

```javascript
var h = new Hls({
  lowLatencyMode: true,
  liveSyncDurationCount: 3,
  liveMaxLatencyDurationCount: 6,
  backBufferLength: 30,
  debug: false
});
h.loadSource('../test.m3u8');
```

补充要求：

- 显示当前 latency、buffered range、video readyState。
- 监听 `MANIFEST_PARSED`，记录 audio track 数量。
- 监听 `BUFFER_CREATED` 或等价 hls.js 事件，确认存在 audio/video 两类 SourceBuffer。
- fatal media error 只做一次 `recoverMediaError()`，重复 fatal 需要展示错误。
- Safari 原生 HLS 仍走 `video.src = playlist_url` fallback。

---

## 2. 自动化 HTTP/ffprobe 验证

新增脚本：`dev-scripts/check_llhls_demuxed.sh`

前置：

```bash
export CHEETAH_CONFIG=./config.yaml
cargo build -p cheetah-server --features "rtsp,hls"
RUST_LOG=info ./target/debug/cheetah-server
```

推流样例：

```bash
ffmpeg -stream_loop -1 -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv \
  -c copy -strict experimental -f rtsp -rtsp_transport udp \
  rtsps://127.0.0.1:8322/live/test
```

脚本检查项：

```bash
BASE="${BASE:-http://127.0.0.1:8088}"
STREAM="${STREAM:-live/test}"

MASTER=$(curl -sf "$BASE/$STREAM.m3u8")
echo "$MASTER" | grep -q "#EXT-X-MEDIA:TYPE=AUDIO"
echo "$MASTER" | grep -q 'AUDIO="audio"'
echo "$MASTER" | grep -q "chunklist_video.m3u8"
echo "$MASTER" | grep -q "chunklist_audio.m3u8"

VIDEO_PL=$(curl -sf "$BASE/$STREAM/chunklist_video.m3u8?uid=1")
AUDIO_PL=$(curl -sf "$BASE/$STREAM/chunklist_audio.m3u8?uid=1")

echo "$VIDEO_PL" | grep -q 'EXT-X-MAP:URI="init_video.mp4'
echo "$AUDIO_PL" | grep -q 'EXT-X-MAP:URI="init_audio.mp4'
echo "$VIDEO_PL" | grep -q "video_part_"
echo "$AUDIO_PL" | grep -q "audio_part_"
echo "$VIDEO_PL" | grep -q "EXT-X-RENDITION-REPORT"
echo "$AUDIO_PL" | grep -q "EXT-X-RENDITION-REPORT"

curl -sf -o /tmp/init_video.mp4 "$BASE/$STREAM/init_video.mp4"
curl -sf -o /tmp/init_audio.mp4 "$BASE/$STREAM/init_audio.mp4"
ffprobe -v error -show_streams /tmp/init_video.mp4 | grep -q "codec_type=video"
! ffprobe -v error -show_streams /tmp/init_video.mp4 | grep -q "codec_type=audio"
ffprobe -v error -show_streams /tmp/init_audio.mp4 | grep -q "codec_type=audio"
! ffprobe -v error -show_streams /tmp/init_audio.mp4 | grep -q "codec_type=video"

VPART=$(echo "$VIDEO_PL" | grep -o 'URI="[^"]*video_part_[^"]*"' | head -1 | sed 's/URI="//;s/"//')
APART=$(echo "$AUDIO_PL" | grep -o 'URI="[^"]*audio_part_[^"]*"' | head -1 | sed 's/URI="//;s/"//')
curl -sf -o /tmp/video_part.m4s "$BASE/$STREAM/$VPART"
curl -sf -o /tmp/audio_part.m4s "$BASE/$STREAM/$APART"

cat /tmp/init_video.mp4 /tmp/video_part.m4s > /tmp/video_combined.mp4
cat /tmp/init_audio.mp4 /tmp/audio_part.m4s > /tmp/audio_combined.mp4
ffprobe -v error -show_packets /tmp/video_combined.mp4 | grep -q "codec_type=video"
! ffprobe -v error -show_packets /tmp/video_combined.mp4 | grep -q "codec_type=audio"
ffprobe -v error -show_packets /tmp/audio_combined.mp4 | grep -q "codec_type=audio"
! ffprobe -v error -show_packets /tmp/audio_combined.mp4 | grep -q "codec_type=video"
```

脚本不能依赖固定 `uid` 以外的随机 stream key；如果 `stream_key_validation=true`，必须从 playlist 中提取带 query 的 URI 后请求。

---

## 3. hls.js / Chrome 验证

新增 Playwright 验证脚本或测试说明：

- 打开 `http://127.0.0.1:8088/live/test/`。
- 等待 hls.js manifest parsed。
- 断言 audio track 数量 >= 1。
- 断言已创建 audio/video 两类 SourceBuffer。
- 等待 `video.readyState >= 2`。
- 播放 10 秒内无 fatal `bufferAppendError`。
- `video.currentTime` 持续前进。
- hls.js latency 稳定在 1-3 秒范围；目标 < 2 秒，但 CI 可放宽到 3 秒避免调度抖动。

Chrome 是主验收目标。Firefox/Edge 做手工或 nightly 验证；Safari 原生 HLS 做手工验证，因为 CI 环境通常不可用。

---

## 4. ffplay 兼容验证

`ffplay` 仍请求主 master playlist：

```bash
export SDL_VIDEODRIVER=dummy
export SDL_AUDIODRIVER=dummy
ffplay -strict experimental http://127.0.0.1:8088/live/test.m3u8 -x 640
```

验收：

- 能解析 master playlist。
- 能加载 `chunklist_video.m3u8` 和 `chunklist_audio.m3u8`。
- 无 `could not find codec parameters`。
- 无 `Invalid NAL unit size`。
- 音频 packet 和视频 packet 都能持续读取。

---

## 5. 回归场景

| 场景 | 验证方式 |
|------|----------|
| TS 容器模式 | 配置 `container: "ts"` 后 `ffplay /live/test.m3u8` |
| 非 LLHLS fMP4 | `ll_hls_enabled=false` 后保持旧 fMP4 行为 |
| video-only 流 | 不生成 audio rendition，hls.js 正常播放画面 |
| audio-only 流 | master 指向 audio chunklist，ffprobe 只看到 audio |
| stream conclude | video/audio chunklist 都输出 `EXT-X-ENDLIST`，pending 请求立即返回 |
| origin mode | 响应不设置 `Set-Cookie`，资源 URL 可被 CDN 缓存 |
| stream key validation | playlist 中所有 media URL 带 `k=`，错误 key 返回 404 |
| legacy URL | `index.m3u8`、`init.mp4`、`part_N.m4s` 映射到 video lane |
| H265 输入 | 生成 `hvc1/hev1` codec string；浏览器播放能力按客户端支持判定 |

---

## 6. 性能指标

| 指标 | 目标 |
|------|------|
| hls.js 端到端延迟 | 目标 < 2s，CI 容忍 < 3s |
| Part 生成延迟 | < 50ms |
| Playlist 生成延迟 | < 5ms |
| 单 stream 内存 | < 50MB |
| 并发播放器数 | >= 100 |

注意：demuxed A/V 会增加一个 audio lane 的 ring、part 缓存和 pending 请求集合，内存评估必须按 video+audio 总和计算。

---

## 7. 完成标准

- [ ] Chrome + hls.js 自动创建独立 video/audio SourceBuffer。
- [ ] Chrome 播放 10 秒无 fatal `bufferAppendError`。
- [ ] `init_video.mp4` / `video_part_N.m4s` 只含 video。
- [ ] `init_audio.mp4` / `audio_part_N.m4s` 只含 audio。
- [ ] Master playlist 包含合法 audio rendition 和 video variant。
- [ ] `ffplay http://127.0.0.1:8088/live/test.m3u8` 可播放音视频。
- [ ] TS、video-only、audio-only、origin mode、stream key validation 回归通过。
- [ ] `cargo test -p cheetah-hls-core`
- [ ] `cargo test -p cheetah-hls-module`
- [ ] `cargo clippy -p cheetah-hls-core`
- [ ] `cargo clippy -p cheetah-hls-module`
- [ ] `cargo build -p cheetah-server --features "rtsp,hls"`
