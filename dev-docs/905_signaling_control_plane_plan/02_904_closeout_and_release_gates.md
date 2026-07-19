# 02 · 904 收尾与发布门禁

## 1. P0 目标

P0 不重写 904 已完成的媒体处理代码，只补齐严格迁移与发布证据。所有证据必须来自同一
Cheetah commit、同一 avcodec revision 和同一候选制品。

## 2. CL904-01：删除直接 `image` 依赖

当前 `image` 只剩 workspace/test fixture 用途，仍违反 904 的直接依赖清理要求。

实施：

- 删除根 `image` workspace dependency，以及 SDK、snapshot、media adapter 等测试 crate
  的 `image.workspace = true`。
- 将测试生成 JPEG 的逻辑替换为小型、许可明确、固定 checksum 的受控 fixture。
- 需要验证解码时使用启用 avcodec feature 的专用 lane；默认 feature 测试只消费 fixture，
  不为测试方便引入媒体库。
- 不把 fixture base64 大段内联到 Rust；存放在相邻 `tests/testdata/` 并记录来源/生成命令。

门禁：

```text
rg -n 'image::|image\.workspace|^image = ' Cargo.toml crates apps
cargo tree -p cheetah-server --no-default-features
```

允许 `ImageFormat` 等 Cheetah 自有类型和变量名 `image`；禁止直接第三方 crate。

## 3. CL904-02：修正 C0–C6

server 当前只暴露 `media-processing`/`media-processing-full`，904 文档中的若干 server
feature 命令不可直接运行。矩阵固定为：

| Lane | 命令范围 | 退出条件 |
| --- | --- | --- |
| C0 | server no-default + 默认 processing crate | 无 avcodec/tonic/SQLite |
| C1 | processing crate `media-processing-audio` | G711/AAC/Opus/MP3 matrix |
| C2 | image + NativeFree | JPEG/PNG decode、算子、snapshot；无 FFmpeg linkage |
| C3 | video + NativeFree | H.264/H.265/MJPEG decode/encode matrix |
| C4 | `media-processing-full` | software/OpenCV、动态库版本 |
| C5 | server `media-processing-full` + 协议 features | 五条集成 E2E |
| C6 | release | artifact、SBOM、license、perf、soak、证据 |

CI 不使用 `--all-features` 替代明确矩阵。C0 必须检查 Cargo tree；C2/C4 检查动态链接；
C1–C5 记录 avcodec 完整 revision 和 preflight。

## 4. CL904-03：五条真实 E2E

| ID | 流程 | 必需证据 |
| --- | --- | --- |
| E2E-IMG | H.264/H.265/MJPEG live → JPEG | Snapshot API、独立 JPEG decoder、尺寸 |
| E2E-FLV | G711/Opus → AAC → RTMP/HTTP-FLV | packets、decoded samples、时间戳 |
| E2E-WEB | AAC/MP3 → Opus → Chrome WHEP | getStats、inbound samples、页面结果 |
| E2E-PRX | RTSP pull H.265/G711 → H.264/AAC → RTMP/HLS | 源/派生/消费三段证据 |
| E2E-HLS | H.264+CEA → 三档 ABR+WebVTT | master、切档、VTT cue、播放器结果 |

测试只使用 loopback、动态端口和仓库 fixture。ffmpeg/ffprobe 可作为外部验证器，不得成为
服务器内部实现。HTTP 2xx、文件非空或任务 Running 不足以通过。

## 5. CL904-04：性能和 24 小时长稳

在固定 CPU/内存/OS 上建立：

- 720p/1080p transcode；
- 三档 ABR；
- 4 路宫格；
- 8 路混音；
- Snapshot burst；
- 五条协议混合负载。

记录 FPS、CPU、RSS、startup/P95 end-to-end latency、queue、drop、worker/job/lease 数。
相对批准基线回退超过 10% 阻断发布，除非报告明确批准并更新基线。

24 小时 workload 必须包含 source reconnect、慢消费者、create/stop、module restart、
registry 暂时不可达和 engine shutdown。结束后 leak report 为空，RSS/handle 数进入稳定区间。

## 6. CL904-05：供应链和证据

- 记录 Cheetah commit、avcodec version/full revision、Rust toolchain、OS/arch。
- 生成 SBOM、license report、Cargo.lock checksum、制品 checksum。
- 证明 Cheetah 直接依赖只有顶层 `avcodec`。
- 记录 NativeFree/Software 的实际动态链接和 FDK-AAC 发行结论。
- 复制 904 模板为 `release_evidence_<version>.md` 并填写全部字段。

P0 最终结论只能是 `PASS` 或 `BLOCKED`。没有 artifact URL/checksum 的项目不得勾选。

## 7. P0 退出条件

- CL904-01..05 均有 owner、revision、命令和 artifact。
- 904 README 全局 DoD 与正式 evidence 一致。
- capability/preflight 与候选制品实际 profile 一致。
- P0 PASS 后才允许 905 控制面进入可发布制品；开发分支不得以 905 工作覆盖 904 blocker。
