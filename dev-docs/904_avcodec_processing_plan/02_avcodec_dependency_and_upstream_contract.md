# 02 · avcodec 依赖与上游契约

## 1. 唯一依赖入口

根 workspace 依赖使用以下形态，最终 revision 取 MP3 上游合入 commit：

```toml
avcodec = {
  version = "<merge revision 对应版本>",
  git = "https://github.com/TimothyWalker6922/avcodec-rs-develop",
  rev = "<包含 MP3 支持的完整 40 位 SHA>",
  default-features = false
}
```

禁止事项：

- 不直接依赖 `avcodec-core-*`、`avcodec-codec-*`、`avcodec-backend-*` 或其 FFI crate。
- 不使用 path override、branch 或 tag 代替不可变 revision。
- 不在多个 Cheetah crate 重复声明不同 avcodec feature；统一由处理模块 feature 汇聚。
- 不在公共结构、错误、日志字段或配置中暴露上游 backend 类型名。

## 2. Cargo feature 契约

应用层固定暴露：

| Feature | 作用 |
| --- | --- |
| `media-processing-audio` | `g711`、`opus`、`fdk-aac`、`audio-resample-rubato` 及 MP3 software decode |
| `media-processing-video` | 视频任务和 AVFrame/Packet adapter |
| `media-processing-image` | 图片任务、快照和 OSD |
| `avcodec-profile-native-free` | 映射 `avcodec/profile-native-free` |
| `avcodec-profile-software` | 映射 `avcodec/profile-software` |
| `media-processing-cpu` | 三项能力 + 两个 CPU profile |

所有 feature 默认关闭。`media-processing-video`/`image` 至少组合一个 profile，否则编译期给出明确错误；audio 可单独构建。任务必须显式选择 profile，默认配置选择 `NativeFree`，不得失败后静默切换 `Software`。

`snapshot` feature 通过 `media-processing-image + avcodec-profile-native-free` 显式选择图片能力；根默认 feature 不包含 snapshot。

## 3. 上游 MP3 前置任务

`UP-01` 在 avcodec-rs 完成：

1. 在公共模型增加 `CodecId::Mp3`，保持稳定 repr 值并同步 FFI schema。
2. 在 software backend 增加 MP3 decoder selection、submit/poll/flush/reset。
3. 为采样率、声道、timebase、truncated packet、flush 尾帧和错误诊断补测试。
4. 让 `AudioTranscoder` 能完成 MP3 → PCM → Opus。
5. 更新 capability/profile 文档、integration guide 和 release notes。
6. 合入上游主分支后记录 merge SHA；Cheetah 不 pin 未合入的个人分支。

如果上游未接受，AUD-03/MP3 保持 Blocked，禁止添加 minimp3、FFmpeg binding 或子进程 fallback。

## 4. 高层 API 使用规则

- 视频 decode/encode/transcode 必须使用 `VideoSdk`、`VideoProfile` 和高层 request/session。
- 图片处理通过高层 image processor session；多个算子按受控 pipeline 顺序执行。
- 音频暂按稳定 API 使用私有 Registry + `AudioTranscoder`；Registry 的构造、backend hint 和 selection report 封装在 adapter 内。
- submit 后循环 poll；`Pending` 进入有界等待/继续推进，不映射为失败。
- 输入结束依次 flush decoder、processor、encoder，直到 EOS；reset 只在显式重启或可恢复 discontinuity 使用。
- 每个 session 固定在一个 blocking worker 内创建、使用和销毁。
- 创建 session 前必须运行 preflight；selection failure 原因转换为稳定 Cheetah error 和 capability diagnostic。

## 5. Codec 与格式映射

处理模块维护穷尽映射：

- Cheetah H264/H265/MJPEG/JPEG/PNG/AAC/Opus/G711A/G711U/MP3/PCM ↔ avcodec `CodecId`
- Cheetah timebase、PTS/DTS/duration/keyframe/discontinuity ↔ avcodec Packet/AudioFrame
- `TrackInfo` width/height/sample-rate/channels/codec-config ↔ decoder/encoder request
- avcodec encoded output重新进入 `AVFrame + TrackInfo`，参数集、时间戳和 Access Unit 再经 `cheetah-codec` 规范化

未知或无法无损表达的格式返回 `Unsupported`，不得使用默认 codec、默认 timebase 或空配置继续运行。

## 6. 供应链与许可

- Cargo.lock、完整 revision、源码 checksum、许可证清单和 SBOM 进入 release artifact。
- CI 检查默认构建无 `avcodec`；处理构建只有顶层 `avcodec` 是 Cheetah 直接依赖。
- NativeFree 制品通过动态链接检查证明未链接 FFmpeg。
- Software 制品记录 avcodec-rs 内部 FFmpeg backend 的库版本和运行时依赖。
- FDK-AAC 单独执行许可证/再分发审核；未获目标发行渠道批准时不得发布包含该 feature 的二进制。
- FFmpeg/ffprobe 可以作为 CI 互操作客户端，但不是生产制品或运行时依赖。
