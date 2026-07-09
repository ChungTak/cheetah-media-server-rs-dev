# Phase 05 — 兼容性与互操作验证

- **状态**: 规划中
- **范围**: 用 ABL 样例、真实客户端和故障样例验证 fMP4 对齐是否成立。
- **完成标准**: 不只单元测试通过，还能在真实输入、真实播放器和非标准数据上稳定运行。

## 5.1 ABL 样例验证

优先构建或提取以下样例：

- ABL 产生的 HTTP-MP4 直播输出。
- ABL 录制的 fMP4 文件。
- ABL H265 fMP4 切片样例。
- ABL AAC/G711 混合流样例。

验证方向：

- Cheetah pull ABL 输出。
- Cheetah demux ABL 录像。
- Cheetah 播放链路输出给 ffmpeg/VLC。

## 5.2 客户端矩阵

- `ffplay`
- `ffmpeg`
- `VLC`
- 浏览器 MSE 样例（如适用）

重点检查：

- 首帧出画时间。
- H265 可播性。
- AAC/G711 音频行为。
- track/config 变化后是否恢复。

## 5.3 fault corpus

增加回归样例：

- 无 `styp`
- 无 `sidx`
- 重复 init
- unknown top-level box
- 半包 / 粘包
- oversized box
- 参数集晚到
- live/replay 时间戳异常样例

样例统一进入：

- `crates/protocols/fmp4/testing/` 的集成夹具
- `crates/protocols/fmp4/fuzz/` 的回归语料

## 5.4 测试与检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec -- fmp4
cargo clippy -p cheetah-fmp4-core
cargo test -p cheetah-fmp4-core
cargo clippy -p cheetah-fmp4-driver-tokio
cargo test -p cheetah-fmp4-driver-tokio
cargo clippy -p cheetah-fmp4-module --tests
cargo test -p cheetah-fmp4-module
cargo test -p cheetah-fmp4-property-tests
(cd crates/protocols/fmp4/fuzz && cargo +nightly fuzz build)
```

## 5.5 通过标准

- ABL 样例可被 Cheetah 稳定 demux 或 pull。
- Cheetah 输出可被至少 `ffplay` 和 `VLC` 连续播放。
- 关键路径测试、property tests、fuzz build 通过。
- 非标准样例失败时有 bounded diagnostic，而不是 panic 或无界内存增长。
