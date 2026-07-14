# 11 · 测试、CI、安全与发布门禁

## 1. 工具链恢复

当前 `rust-toolchain.toml` 指定的 `1.96.1` 在审计环境不可获取，默认 cargo 命令无法启动。S0 固定到已经验证可编译当前 workspace 的 Rust `1.94.1`，并在 CI 镜像验证 rustfmt/clippy 可安装。未来升级单独提交，不与 API 功能 PR 混合。

完成工具链调整后先运行：

```bash
rustc --version
cargo fmt --check
cargo check -p cheetah-media-api
cargo test -p cheetah-sdk signal_contracts
```

## 2. 测试层级

| 层 | 内容 | 外部依赖 |
| --- | --- | --- |
| L0 domain unit | DTO、校验、错误、capability、幂等 | 无 |
| L1 provider integration | engine/record/snapshot/proxy/RTP provider | 内存/临时目录 |
| L2 adapter contract | native route、auth、ZLM golden | 无公网 |
| L3 protocol loopback | RTP/RTSP/RTMP/WHIP 实际 socket | localhost |
| L4 signal production | GB/ONVIF/HomeKit/Matter 媒体流程 | localhost |
| L5 external interop | 可选真实客户端/server | 手工或专用 CI |

发布门禁要求 L0–L4 全绿；L5 根据环境单独记录，不得用 L5 不可用跳过 L0–L4。

## 3. 必测并发与生命周期

- 同一 stream 并发两个 publisher，只有一个成功。
- provider 注册与 adapter 请求并发，不出现 stale Arc/panic。
- module restart 时旧 session 有终态，新 generation 可服务。
- slow subscriber、slow webhook、slow file download 互不阻塞媒体热路径。
- queue、session、task、file、proxy、RTP store 达到上限时返回 Busy/Unavailable。
- cancel 与 completion 竞态只产生一个终态事件。
- duplicate stop/delete/kick 保持规定的幂等语义。

## 4. HTTP 与兼容测试

native：逐 route 覆盖 method/path/body/query/header/auth/error。ZLM：64 route catalog、字段 golden、错误码、secret、布尔/数字/时间别名。测试必须比较响应 JSON 的字段位置和类型，不能只断言 `code=0`。

adapter 组合测试：

- native on/ZLM on。
- native on/ZLM off。
- native off/ZLM on。
- 自定义 prefix。
- provider feature 缺失。
- module restart 后 mount 更新。

## 5. 安全测试

| ID | 风险 | 期望 |
| --- | --- | --- |
| SEC-01 | 无鉴权踢流/删文件 | 401/403 或 ZLM -100 |
| SEC-02 | 跨 vhost/app 权限 | 拒绝且不泄露存在性 |
| SEC-03 | 文件路径穿越 | 400/403，无文件读取 |
| SEC-04 | webhook SSRF | loopback/link-local/metadata 目标拒绝 |
| SEC-05 | FFmpeg 参数注入 | typed validation 拒绝 |
| SEC-06 | secret 日志泄漏 | captured logs 无敏感值 |
| SEC-07 | 大 body/分页/队列 | 有上限且无 OOM |
| SEC-08 | URL Host 注入 | 使用配置 public host |
| SEC-09 | 下载 handle 猜测/过期 | 404/403 |
| SEC-10 | webhook 重试风暴 | 有界并熔断 |

## 6. 每阶段最低命令

```bash
cargo fmt --check
cargo clippy -p <changed-crate>
cargo test -p <changed-crate>
```

共享层变更增加：

```bash
cargo test -p cheetah-media-api
cargo test -p cheetah-sdk
cargo test -p cheetah-engine
cargo test -p cheetah-media-module
```

RTP/record/proxy/snapshot 按受影响 module 增加测试。不要例行使用 `--all-features`，使用明确的交付 profile 和受影响 feature 组合。

## 7. Server 组合门禁

至少检查：

```bash
cargo check -p cheetah-server
cargo check -p cheetah-server --no-default-features --features 'rtmp,rtsp,http-flv,hls,rtp,record,webrtc,mp4,fmp4,srt'
```

若新增 `media-control-full` feature，第二条改为该 profile，并保留一次显式 feature 展开检查，防止 profile 漏依赖。

## 8. 发布验收报告

每阶段 PR 附：任务 ID、能力前后状态、测试命令及结果、未完成项、配置/迁移影响。S10 生成总报告：

- capability 实际矩阵。
- native route 数量和测试覆盖。
- ZLM 64 route 状态及 hook 状态。
- 四类 production contract 结果。
- 安全测试结果。
- 已知限制和明确 Unsupported。

任何 P0 未完成、测试不能编译、capability 说谎、鉴权绕过或 production contract 依赖 fake，均阻止发布。

