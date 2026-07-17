# 13 · 测试、CI 与发布门禁

## 1. CI 构建矩阵

| Lane | Feature | 目标 |
| --- | --- | --- |
| C0 | 默认 features | 不编译 avcodec/processing |
| C1 | audio | G711/Opus/AAC/MP3 音频矩阵 |
| C2 | image + NativeFree | 图片算子、JPEG、快照 |
| C3 | video + NativeFree | H.264/H.265/MJPEG matrix |
| C4 | image/video + Software | software profile matrix、动态库检查 |
| C5 | media-processing-cpu | 所有处理、混流、字幕、协议 E2E |
| C6 | release | SBOM、license、制品、长稳和报告 |

C0 必须用 Cargo tree 证明没有 `avcodec`。C2 NativeFree 制品必须用平台链接检查证明没有 FFmpeg shared library。C4/C6 记录实际链接库版本。

## 2. 分层测试

### `cheetah-codec`

- CEA/SEI/WebVTT 纯单元测试、属性测试、fuzz。
- AVFrame/TrackInfo 新枚举和 timebase roundtrip。
- 不运行真实网络和 avcodec session。

### Processing module

- fake RuntimeApi/StreamManager 做 Job 生命周期和错误回滚。
- 真实 avcodec profile 做 codec/image matrix。
- worker ownership、Pending、flush/reset、backpressure、配额和取消。

### Protocol modules

- production processing provider + 本地协议对端。
- Auto/Transcode/Passthrough、能力缺失、共享复用和清理。
- 不在 core 测试中引入处理依赖。

## 3. 必需功能场景

- IMG：JPEG/PNG/H.264/H.265/MJPEG 输入、全部媒体图片算子、JPEG 输出、PNG Unsupported。
- AUD：七条音频矩阵、采样率/声道/时长/PTS、flush 尾帧。
- VID：required video matrix、码率/fps/尺寸/GOP、overlay、PLI/FIR。
- ABR：1–4 档、原子创建、master 一致性、切档。
- MIX：16 路音频边界、9 路视频边界、失联/恢复/同步。
- SUB：608/708、H.264/H.265 SEI、WebVTT/HLS。
- API：Rust SDK、Native HTTP、鉴权、分页、幂等、generation。
- MIG：旧 FFmpeg API/YAML/route 消失，旧 TranscodePolicy 拒绝。

## 4. E2E 与互操作

- H.264/H.265/MJPEG live → Snapshot JPEG。
- G711/Opus → AAC → RTMP/HTTP-FLV。
- SRT/RTSP H.264 + AAC/MP3 → H.264 + Opus → Chrome WHEP。
- RTSP pull H.265/G711 → H.264/AAC derived → RTMP/HLS。
- H.264 + CEA → 三档 ABR + WebVTT → HLS player。

验证器必须检查真实 packets/frames/decoded samples，而不是只检查 HTTP 2xx、session 存活或非空文件。FFmpeg/ffprobe 只能作为外部客户端；浏览器路径记录 getStats 和媒体证据。

## 5. 性能与长稳

- 建立固定 CPU 机器/fixture 的 720p、1080p、三档 ABR、4 路宫格、8 路混音基线。
- 记录 fps、CPU、RSS、startup latency、端到端 latency、queue、drop。
- 同一基准后续 CPU/内存/延迟回退超过 10% 阻断发布，除非报告中批准并更新基线。
- 24 小时混合负载包含源流重连、消费者抖动、Job create/stop、module restart。
- RSS 和 live resource 数必须进入平台稳定区间；结束后 leak report 为空。

## 6. 命令门禁

每个任务至少执行：

```text
cargo fmt --check
cargo clippy -p <changed-crate> -- -D warnings
cargo test -p <changed-crate>
```

公共层/codec/runtime 变更继续运行反向依赖。阶段门禁另外执行：

```text
cargo build -p cheetah-server --no-default-features
cargo tree -p cheetah-server --no-default-features
cargo build -p cheetah-server --no-default-features --features media-processing-audio
cargo build -p cheetah-server --no-default-features --features media-processing-image,avcodec-profile-native-free
cargo build -p cheetah-server --no-default-features --features media-processing-video,avcodec-profile-native-free
cargo build -p cheetah-server --no-default-features --features media-processing-image,media-processing-video,avcodec-profile-software
cargo build --release -p cheetah-server --no-default-features --features media-processing-cpu
```

具体 crate/test 名在实现落地后补入 CI 脚本，不使用 `--all-features` 代替明确矩阵。

## 7. 发布阻断项

- revision 未固定、MP3 使用未合入 fork、license/SBOM 缺失。
- 默认制品编译 avcodec 或 NativeFree 链接 FFmpeg。
- 能力报告包含未通过真实 E2E 的 operation。
- 任一队列/缓存/输入数/像素率无上限。
- admission deny、cancel、restart、shutdown 有资源泄漏。
- PNG/硬件/SVC 等未交付能力被文档或 API 宣称可用。
- 24 小时 soak、五条协议 E2E 或 release evidence 未完成。
