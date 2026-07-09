# Phase 06 — 外部互操作实体测试基础设施

- **状态**: 第十六轮已落地（SVC L3T3 + DTMF telephone-event fixture）。下一轮聚焦 dual-stream simulcast 媒体面验证、Windows 弱网自动化、SVC 媒体面层选验证（依赖 Playwright getStats）。

## 已完成（Phase 06 第十六轮）

- `crates/protocols/webrtc/module/tests/fixtures/zlm/svc_offer.sdp`：VP9 SVC L3T3 offer fixture（PT 98 + RTX 99，`a=scalability-mode:L3T3`、`profile-id=0`、保留 RTX + FID ssrc-group）。L3T3 = 3 spatial layers × 3 temporal layers，是浏览器在 SVC 模式下下发的标准形态。
- `crates/protocols/webrtc/module/tests/fixtures/zlm/dtmf_audio_offer.sdp`：DTMF 音频 offer fixture（opus PT 111 + telephone-event PT 110/126，48 kHz 与 8 kHz 双 rtpmap，`a=fmtp:* 0-16` DTMF 事件范围）。覆盖 PSTN 网关接入与浏览器电话拨号场景。
- `tests/zlm_sdp_fixtures.rs` 新增 6 条 fixture 测试：
  - SVC: well-formed、`a=scalability-mode:L3T3` + VP9 PT、RTX/FID 保留（3 条）。
  - DTMF: well-formed、双 telephone-event rtpmap + `0-16` fmtp、opus 仍是首选 PT（m= 行第一个 PT）（3 条）。
- 与第十五轮的 screen-share fixture 配合，cheetah 现在覆盖 8 种 ZLM 风格 SDP 形态的回归（WHIP / WHEP answer、TCP / IPv6 / TURN candidate offer、DataChannel / SCTP answer、GB28181 play、H.264-only、simulcast、TCP fallback、低延迟、screen-share、SVC、DTMF）。

## 已完成（Phase 06 第十五轮）

- `crates/protocols/webrtc/module/tests/fixtures/zlm/screen_share_offer.sdp`：屏幕共享 offer fixture（audio + video 两段共用 `screen-share` msid 流 id、video 段含 `a=content:slides` + `video-content-type` extmap、保留 RTX + FID ssrc-group）。
- `tests/interop_harness.rs::assertions` 新增 msid helper：
  - `MsidEntry { stream_id, track_id }`：单条 `a=msid:<stream> <track>` 的解析结果。
  - `extract_msids(sdp) -> Vec<MsidEntry>`：按声明顺序返回所有 msid 条目；malformed 行（缺空格、空字段）跳过不报错。
  - `assert_msid_stream_present(sdp, stream_id)`：报告期望 stream 不在场时返回带已观察到的 stream 列表的错误字符串（方便写 `failure.txt`）。
  - 4 条新单元测试覆盖 happy path、malformed 行 skip、stream 匹配通过 / 不在场报告、空 SDP 报告。
- `tests/zlm_sdp_fixtures.rs` 新增 4 条 screen-share fixture 测试：well-formed、audio + video msid 同流、`a=content:slides` + extmap 在场、RTX/FID 保留。

## 已完成（Phase 06 第十四轮）

- `crates/protocols/webrtc/module/tests/fixtures/zlm/tcp_fallback_answer.sdp`：ZLM 在 client 走 TCP fallback 时返回的 answer fixture。`m=video TCP/TLS/RTP/SAVPF` proto、tcptype passive 候选（host + srflx）、保留 `a=rtpmap:97 rtx/90000` + `a=ssrc-group:FID`、TCP 上仍开 `transport-cc` 反馈。
- `tests/zlm_sdp_fixtures.rs` 新增 3 条 TCP fallback 测试：
  - `tcp_fallback_answer_passes_well_formed_check`：`assert_answer_well_formed` 通过（验证 RFC 7850 TCP 变体不会被 harness 误判）。
  - `tcp_fallback_answer_uses_tcp_proto_and_passive_candidates`：m= 行 proto = TCP/TLS/RTP/SAVPF、≥1 TCP 候选、0 UDP 候选、≥1 tcptype passive。
  - `tcp_fallback_answer_keeps_rtx_for_recovery`：RTX rtpmap + apt fmtp + FID ssrc-group 都在场。
- `.github/workflows/webrtc-interop-nightly.yml::weak-network-default` 从单 profile 升级为 `loss-5 + reorder` 矩阵：`fail-fast: false` 让一个 profile 失败不阻塞另一个；`strategy.matrix` 共用一份 `Build` 步骤但每个 profile 独立 `Run` + `Upload artifacts`；artifact name 带 profile 后缀（`webrtc-interop-weak-default-loss-5` / `-reorder`）。这样默认每日 nightly 已经覆盖丢包 + 重排两种弱网场景，full 6-profile matrix 仍只在 manual dispatch 跑。

## 已完成（Phase 06 第十三轮）

- `crates/protocols/webrtc/module/tests/cheetah_to_janus_interop.rs` 新增真实端到端 ignored 测试 `cheetah_drives_janus_echotest`：
  - 与 `cheetah_to_zlm_interop.rs` 同模式：tokio 原生 `TcpStream` 写 HTTP/1.1，不引入 reqwest 依赖；超时 / 关闭 write half / 读 to-end 三件套都按 harness 约定走。
  - 三段式 Janus REST 握手：`POST /janus` 创建 session（提取 `data.id`）→ `POST /janus/<session>` 附加 `janus.plugin.echotest`（提取 `data.id` 当 handle）→ `POST /janus/<session>/<handle>` 发 echotest message（接受 `ack` / `event` / `success` 三种响应类型）。
  - 三步响应分别落到 `step1-create.json` / `step2-attach.json` / `step3-message.json` artifact，便于事后排查；任意一步非 2xx 或 JSON 解析失败立即 `failure.txt` + panic。
  - 用本地静态 `AtomicU64` 计数生成 transaction id，避免引入 `rand` 依赖。
- `.github/workflows/webrtc-interop-nightly.yml` 增加 “Run cheetah↔Janus ignored end-to-end test” 步骤，与既有的 ZLM / Pion 端到端步骤并列。
- `crates/protocols/webrtc/module/tests/fixtures/zlm/low_latency_offer.sdp` 新 fixture：低延迟 video offer（`playout-delay` + `video-timing` + `transport-cc` + `goog-remb` 全套）。
- `tests/zlm_sdp_fixtures.rs` 新增 3 条 low-latency fixture 测试：well-formed、`playout-delay` + `video-timing` extmap 在场、`transport-cc` + `goog-remb` rtcp-fb 同时存在。

## 已完成（Phase 06 第十二轮）

- `crates/protocols/webrtc/module/tests/cheetah_to_pion_interop.rs` 新增真实端到端 ignored 测试 `pion_publish_to_cheetah_whip`：
  - 用 `std::process::Command` 跑 `WEBRTC_INTEROP_PION_BIN` 指向的 Pion helper 二进制；为了不引入 `tokio/process` feature，用 `tokio::task::spawn_blocking` 包起来 + `tokio::time::timeout` 兜底，超时直接放弃 join（OS 进程会随测试 runner 一起回收）。
  - 把 helper 的 stdout / stderr 写到 `peer.log` artifact；非零退出立即 `failure.txt` + panic。
  - 解析 helper 写出的 `peer-stats.json`，断言 `first_keyframe_ms / nacks_sent / nacks_received / bytes_sent / bytes_received` 五个字段都存在（main.go schema）。
  - env 缺失（`WEBRTC_INTEROP_PION_BIN` 或 `WEBRTC_INTEROP_ZLM_WHIP_URL`）时 harness 自动 skip。
- `.github/workflows/webrtc-interop-nightly.yml` 增加 “Run cheetah↔Pion ignored end-to-end test” 步骤，与既有的 ZLM 端到端步骤并列；按 `inputs.run_filter` 接收过滤参数。
- `crates/protocols/webrtc/module/tests/fixtures/zlm/simulcast_offer.sdp` 新 fixture：3 层 simulcast offer（`hi;mid;lo`），含 `urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id` + `repaired-rtp-stream-id` 扩展、`a=rid:` 三行 + `a=simulcast:send hi;mid;lo`、VP8 + RTX 双 PT。
- `tests/interop_harness.rs::assertions` 新增 simulcast helpers：
  - `SimulcastRids { send: Vec<String> }`：`extract_simulcast_rids(sdp)` 解析 `a=simulcast:send <rids>`，按声明顺序返回；缺失则返回 `None`。
  - `assert_simulcast_layers(sdp, required)`：层数不足返回带列表的错误字符串。
  - 6 条新 harness 单元测试覆盖 happy path（3 层）、2 层 offer、缺失 line、阈值通过 / 不足、缺 line 时报告。
- `tests/zlm_sdp_fixtures.rs` 新增 4 条 simulcast fixture 测试：well-formed、3 层 RID 顺序、必需扩展头、`a=rid:` 行计数 = 3。
- nightly CI workflow 新增 `weak-network-default` job：默认调度（`if: github.event_name == 'schedule'`）单 profile 跑 `loss-5`；既有的 `weak-network` 矩阵 job 仍然只在 `workflow_dispatch` 上跑全部 6 个 profile。这样每日 nightly 就有最小弱网回归覆盖，full matrix 留给运营手动触发。

## 已完成（Phase 06 第十一轮）

- `crates/protocols/webrtc/module/tests/cheetah_to_zlm_interop.rs` 新增真实端到端 ignored 测试 `cheetah_offer_to_zlm_whip`：
  - 起 `WebRtcModule` in-process（与 `cheetah_self_interop.rs` 同样的 builder）
  - cheetah 本地 WHIP 答复满足 201 + `assert_answer_well_formed` 才继续；不满足时写 `failure.txt` 并 panic
  - 用 tokio 原生 `TcpStream` 实现 `http_post_sdp`（避免引入 reqwest 依赖；HTTP/1.1 + `Connection: close` + `Content-Length`），把同一份 offer SDP POST 到 `WEBRTC_INTEROP_ZLM_WHIP_URL`
  - ZLM 答复也走 `assert_answer_well_formed`；非 2xx 或 SDP 不合规即写 `failure.txt`
  - artifact 三份 SDP（`request-offer.sdp` / `cheetah-answer.sdp` / `zlm-answer.sdp`）落盘到 `target/webrtc-interop/cheetah_offer_to_zlm_whip/`，方便事后对比 cheetah 与 ZLM 的字段差异
  - env 缺失或 ZLM 不可达时 harness 自动 skip / 写 failure.txt，不会让 nightly job 假绿
- `.github/workflows/webrtc-interop-nightly.yml` 增加 “Run cheetah↔ZLM ignored end-to-end test” 步骤，与既有的 `interop` ignored 步骤并列；按 `inputs.run_filter` 接收 manual dispatch 时的过滤参数。
- `crates/protocols/webrtc/module/tests/fixtures/zlm/h264_only_offer.sdp` 新 fixture：H.264-only video offer（PT 102 + RTX 103，`packetization-mode=1`、`profile-level-id=42001f`、单 codec PT），覆盖嵌入式设备 / GB28181 网关常见的 single-codec 形态。
- `crates/protocols/webrtc/module/tests/fixtures/zlm/gb28181_play_answer.sdp` 新 fixture：ZLM 把 GB28181 入流转发到 WHEP 时的 answer（`s=ZLMediaKit-GB28181`、单 m=video sendonly H.264、msid `gb gb28181-video`）。
- `tests/zlm_sdp_fixtures.rs` 新增 5 条 fixture 测试：H.264-only 单 codec + RTX、`packetization-mode=1` + `profile-level-id`、GB28181 答复 well-formed、video-only + msid + sendonly 三件套、ZLM-specific session name 标记。
- `dev-docs/plans-27-webrtc-zlm2/interop-weak-network/WINDOWS.md` 给出 Windows / macOS 弱网等价方案：Clumsy（含 profile 到 `--drop / --out-of-order / --bandwidth` 的映射表）、`pktmon` 抓包配合、macOS Network Link Conditioner 用法、self-hosted Windows runner 的 CI 接入建议。

## 已完成（Phase 06 第十轮）

- `tests/fixtures/zlm/datachannel_answer.sdp` 新增 DataChannel/SCTP 答复 fixture：
  - 三段式 BUNDLE：audio (`mid:0`) + video (`mid:1`) + application (`mid:2`)
  - SCTP 段：`m=application 9 UDP/DTLS/SCTP webrtc-datachannel` + `a=sctp-port:5000` + `a=max-message-size:262144`
  - audio + video 段沿用 ZLMediaKit WHEP 规范（sendonly + msid + ssrc + FID for RTX）
- `tests/zlm_sdp_fixtures.rs` 新增 4 条 DataChannel 测试：
  - `datachannel_answer_passes_well_formed_check`：`assert_answer_well_formed` 直接通过
  - `datachannel_answer_includes_application_section_with_sctp_port`：检查 `m=application UDP/DTLS/SCTP webrtc-datachannel` + `a=sctp-port` + `a=max-message-size`
  - `datachannel_answer_bundles_three_sections`：BUNDLE 行 `0 1 2` + 三个 `a=mid:` 行
  - `datachannel_answer_max_message_size_is_within_default_cap`：max-message-size ≤ 262144（驱动默认上界，ZLM 不应越界）
- `.github/workflows/webrtc-interop-nightly.yml` 新增 `weak-network` job（`needs: interop`、`if: github.event_name == 'workflow_dispatch'`）：
  - 6-profile 矩阵（`loss-1 / loss-5 / loss-10 / loss-20 / reorder / bw-cap`）+ `fail-fast: false` 让一个 profile 失败不阻塞其他 profile
  - `cargo test --no-run` 先编译再启 netem，缩短 qdisc 应用窗口
  - `sudo -E env "PATH=$PATH" run-netem.sh ${matrix.profile} -- cargo test ... weak_network_nack_recovery` 在 netem 包裹下跑 ignored 测试
  - 上传 `target/webrtc-interop-weak/` artifact 14 天保留
  - 仅 manual dispatch 触发（避免每天用 root 跑 qdisc 影响 runner 稳定性）

## 已完成（Phase 06 第九轮）

- `dev-docs/plans-27-webrtc-zlm2/interop-cheetah-server/` 新增 cheetah-server 真实可构建镜像目录：
  - `Dockerfile`：multi-stage（`rust:1.83-slim` builder → `debian:12-slim` runtime）；`--mount=type=cache,target=/usr/local/cargo/registry` + `target` cargo 缓存让重建增量化；`cargo build --release -p cheetah-server --features webrtc` 产出二进制；`useradd --uid 1000 cheetah` 非 root 运行；`CHEETAH_CONFIG=/etc/cheetah/config.yaml`、`RUST_LOG=info,cheetah=debug` 默认 env；`EXPOSE 1935 1936 554 8000/udp 8088 8891` 文档级端口列表。
  - `interop.yaml`：互操作 lab 用 config — RTMP + WebRTC 默认开（fewer toggles）、handshake_timeout 缩短到 5s（互操作失败暴露快）、`server_label: "cheetah-interop-lab"` 让 SMS API 响应可识别；其他参数贴近 `config.example.yaml`。
  - `README.md`：`docker build -f .../Dockerfile .`（workspace 根上下文）、`docker run --network host -v interop.yaml:...` 单容器启动、与 ZLM 共存时端口冲突的注意事项。
- `dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml` 新增 `cheetah-server` service（`profiles: [cheetah]`），`build.context: ../..` 指回 workspace 根，`build.dockerfile` 指向新 Dockerfile；`network_mode: host`、挂载 `interop.yaml` 到 `/etc/cheetah/config.yaml:ro`、`RUST_LOG` 可被 env 覆盖。
- `crates/protocols/webrtc/module/tests/cheetah_self_interop.rs` 新增 2 条非 ignored 自闭环测试：
  - `cheetah_self_whip_answer_passes_assertion_helpers`：起 `WebRtcModule` in-process，POST WHIP offer，verify cheetah 答复满足 `assert_answer_well_formed`、不带 relay candidate（无 TURN 配置）。
  - `cheetah_self_whip_answer_carries_required_attributes`：cheetah 答复必须含 `v=0`、`a=group:BUNDLE`、`a=fingerprint:`、至少一个 `a=mid:`、`a=ice-ufrag:` + `a=ice-pwd:` ICE 凭据；防止 str0m 默认输出漂移。
- `.github/workflows/webrtc-interop-nightly.yml` 增加 “Run cheetah self-loopback interop sanity tests” 步骤，在 ZLM fixture sanity 之后、ignored interop 之前运行；任何一步失败立即让 nightly job 失败，避免在外部环境噪音里淹没回归。
- `dev-docs/plans-27-webrtc-zlm2/interop-runner.md` 更新 “docker-compose 一键起” 段：增加 `cheetah` / `gstreamer` / `janus` profile 的 up + exec 命令；helper 脚本骨架段补 `interop-cheetah-server/` 目录树。

## 已完成（Phase 06 第八轮）

- `tests/interop_harness.rs::assertions` 新增候选解析 helper：
  - `CandidateCounts { host, srflx, prflx, relay, tcp, udp, ipv4, ipv6 }`：候选类型 + 传输 + 地址族的桶式计数；`total()` 返回有效候选总数。
  - `count_candidates(sdp)`：纯字符串解析 `a=candidate:` 行，宽松对待格式不规整的输入（如截断、缺字段），符合 nightly lab 中遇到的脏 SDP 真实情况。
  - `assert_candidate_types_present(sdp, require_host, require_srflx, require_relay)`：把候选数据落到具体断言；缺哪类返回带类型名的错误字符串方便写 `failure.txt`。
  - 4 条新单元测试：传输与类型分桶、忽略 partial 行、必需类型断言通过、缺失 relay 报错。
- `tests/fixtures/zlm/` 新增三份 offer fixture：
  - `tcp_candidate_offer.sdp`：ICE TCP 候选（RFC 6544）— 包含 `tcptype active`、`tcptype passive`、TCP srflx
  - `ipv6_candidate_offer.sdp`：纯 IPv6 候选 — 包含 link-local (`fe80::`)、global v6 (`2001:db8::`)、v6 srflx
  - `turn_relay_offer.sdp`：UDP 含 host + srflx + relay（带 `raddr` / `rport`）
- `tests/zlm_sdp_fixtures.rs` 新增 4 条非 ignored 测试：TCP fixture 含至少 2 TCP 候选 + active/passive、IPv6 fixture 含 link-local + global、TURN fixture 含 raddr/rport、跨 fixture 的候选数器 sanity。
- `dev-docs/plans-27-webrtc-zlm2/interop-gstreamer-helper/` 升级到真实可构建镜像：
  - `Dockerfile`：`debian:12-slim` 上 apt 安装 GStreamer 全套（`tools` + `plugins-{base,good,bad,ugly}` + `libav` + `nice`）；非 root 1000:gst 用户运行；`WEBRTC_INTEROP_ARTIFACT_DIR=/artifacts`、`GST_DEBUG=3,webrtc*:5` 默认值。
  - `entrypoint.sh`：`whip|whep` 双模 — `whip` 用 `videotestsrc → vp8enc → rtpvp8pay → whipclientsink`，`whep` 用 `whepclientsrc → rtpvp8depay → vp8dec → fakesink`；`timeout` 包裹 + `tee peer.log` 落盘，`gst-launch-1.0 -e` 使 SIGTERM 触发 EOS 干净退出。
- `dev-docs/plans-27-webrtc-zlm2/interop-janus-helper/` 升级到真实可构建镜像：
  - `Dockerfile`：派生自 `canyan/janus-gateway:latest`；apt 加 `curl + jq + bash`；保留默认入口。
  - `smoke.sh`：三段式 `create / attach / message` REST 调用，按 `step1-create.json` / `step2-attach.json` / `step3-message.json` 写到 artifact 目录；用 `jq` 解 session_id / handle_id；失败回写 stderr。
- `dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml` 新增 `gstreamer-helper` + `janus-helper` 两个 service（`profiles: [gstreamer]` / `profiles: [janus]`），与既有 `zlmediakit` / `pion-helper` / `playwright` 共用 host network 与同名 env 契约；GStreamer service `depends_on zlmediakit (healthy)`。

## 已完成（Phase 06 第七轮）

- `crates/protocols/webrtc/module/tests/fixtures/zlm/`：新增两个 ZLM 风格答复 SDP fixture
  - `whip_answer.sdp`：WHIP 答复（`s=ZLMediaKit`、`a=group:BUNDLE 0 1`、`mid:0/1` audio + video、`a=recvonly`、opus + VP8/RTX、含 `a=rtcp-mux`、`a=rtcp-rsize`、`a=ice-options:trickle`、`a=fingerprint:sha-256`）
  - `whep_answer.sdp`：WHEP 答复（同上但 `a=sendonly`、含 `a=msid:`、`a=ssrc:`、video `a=ssrc-group:FID rtx-pri rtx-sec`）
- `crates/protocols/webrtc/module/tests/zlm_sdp_fixtures.rs`：非 ignored 测试套件，固定每次 `cargo test` 都跑：
  - `zlm_whip_answer_passes_well_formed_check`、`zlm_whep_answer_passes_well_formed_check`：assertion helpers (`assert_answer_well_formed`) 在两份 fixture 上都通过
  - `zlm_answers_carry_required_zlm_specific_fields`：`s=ZLMediaKit`、`a=group:BUNDLE 0 1`、`a=mid:0/1`、`a=rtcp-mux/rsize`、`a=ice-options:trickle` 都存在
  - `zlm_whep_answer_lists_send_direction_and_msid`：WHEP 答复必须 `sendonly` 并至少含 2 个 `a=msid:`
  - `zlm_whep_video_ssrc_group_is_fid_for_rtx`：RTX 必须 `a=ssrc-group:FID`
  - `zlm_whip_answer_is_recvonly_and_rejects_offer_well_formed_via_offer_helper`：WHIP 答复是 `recvonly`，并验证 `assert_offer_well_formed` 只检查形态不检查方向（契约文档化）
  - `interop_thresholds_default_values_are_sane`：`InteropThresholds::default()` 阈值 sanity（first_keyframe ≤ 5s、max_rtt ≤ 2s 等）
- `dev-docs/plans-27-webrtc-zlm2/interop-gstreamer-helper/README.md`：把 GStreamer `webrtcbin` 的 WHIP / WHEP `gst-launch-1.0` 调用、`GST_DEBUG` 抓 log 的写法、apt 依赖清单、docker-compose 集成草图全部写齐；不依赖额外 binary，运行时直接用 `gstreamer1.0-plugins-bad-apps` 的 `whipclientsink` / `whepclientsrc`。
- `dev-docs/plans-27-webrtc-zlm2/interop-janus-helper/README.md`：把 `canyan/janus-gateway:1.x` 的启动参数、`/janus` REST 三段式握手（create / attach / message）、`echotest` plugin 的预期消息格式写齐；docker-compose `profiles: [janus]` 草图。
- `.github/workflows/webrtc-interop-nightly.yml`：在 ignored 互操作步骤之前插入 “Run ZLM answer SDP fixture sanity tests” 步骤，确保 fixture 与 assertion helpers 之间的契约在 nightly 跑前先验证；fixture 变更与 helper 变更不会互相掩盖回归。

## 已完成（Phase 06 第六轮）

- `dev-docs/plans-27-webrtc-zlm2/interop-pion-helper/` 新增 Pion helper 骨架：
  - `Dockerfile`：基于 `golang:1.22-alpine` 多阶段构建，最终 alpine + 非 root 运行；`ENTRYPOINT` 指向 `/usr/local/bin/cheetah-pion-helper`。
  - `main.go`：约 200 行的 WHIP/WHEP 双模 helper（pion/webrtc v3）；自动 ICE gather 完成后 POST SDP；`WHEP` 模式记录第一个 RTP 包的延迟；运行时把 `peer-stats.json` 写到 `WEBRTC_INTEROP_ARTIFACT_DIR`。
  - `go.mod`：固定 `pion/webrtc v3.2.40`，与 nightly 行为对齐。
  - `README.md`：本地 `docker build` / `docker run` 命令、artifact schema 说明。
- `dev-docs/plans-27-webrtc-zlm2/interop-playwright/` 新增 Playwright 浏览器 spec 骨架：
  - `whip-whep.spec.ts`：两条测试（WHIP publish + WHEP play），用 `page.evaluate` 执行 `getUserMedia → createOffer → POST /whip → setRemoteDescription`，再用 `getStats()` 抓 `outbound-rtp` / `inbound-rtp` / `transport` 字段写入 `<artifact-dir>/{whip|whep}-stats.json`。`test.skip` 在 env 缺失时跳过。
  - `playwright.config.ts`：headless chromium、`--use-fake-{ui,device}-for-media-stream`、json reporter 落到 `WEBRTC_INTEROP_ARTIFACT_DIR/playwright-report.json`。
- `dev-docs/plans-27-webrtc-zlm2/interop-weak-network/` 新增 `run-netem.sh`（root 包装；`loss-1 / loss-5 / loss-10 / loss-20 / reorder / bw-cap` 6 个 profile；trap cleanup 删除 qdisc）+ `README.md`（Linux 用法、Windows / macOS 替代、profile ↔ NACK / BWE 阈值对照）。
- `dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml` 升级：
  - `pion-helper` service 改用本地 `build.context: ./interop-pion-helper`，固定 `image: cheetah-pion-helper:dev` tag，挂载 `./interop-pion-helper/artifacts:/artifacts`，与本地 spec 同源。
  - `playwright` service 增加 `working_dir: /work` + `volumes: ./interop-playwright:/work, ./interop-playwright/artifacts:/artifacts`，docker compose exec 即可跑 `npx playwright test`。
- `crates/protocols/webrtc/module/tests/interop.rs` 新增 ignored 测试 `zlm_answer_sdp_validation`：当操作员把 ZLM 实际返回的 answer SDP 文件放到 `target/webrtc-interop/zlm_answer_sdp_validation/response-answer.sdp` 后，测试会用 `assertions::assert_answer_well_formed` 验证；不存在或不合法时写 `failure.txt` 帮助诊断。是 assertion helpers 与 ignored 测试体闭环的最小例子。

## 已完成（Phase 06 第五轮）

- `module/tests/interop_harness.rs::assertions` 模块新增可复用的媒体面 assertion 工具：
  - `InteropThresholds`：`first_keyframe`、`max_rtt`、`min_nacks_under_loss`、`min_bwe_bps` 默认阈值，对齐 phase-06 文档里的成功断言标准。
  - `assert_offer_well_formed` / `assert_answer_well_formed`：SDP 形态检查（`v=0`、至少一个 `m=`、answer 必须含 `a=fingerprint:`）。
  - `assert_first_keyframe_within` / `assert_nack_engaged` / `assert_bwe_above`：把媒体面阈值落到具体函数，失败时返回可写进 `failure.txt` 的字符串。
  - 8 条单元测试覆盖 happy path、缺 `v=0`、缺 `m=`、缺 `fingerprint`、超阈值、NACK 不足、BWE 不足。
- 在等待真实媒体路径打通前，这些 helpers 让 ignored 测试可以在拿到 SDP / stats 后立即写出失败原因，避免靠 `assert_eq!` 输出难以诊断。

## 已完成（Phase 06 第四轮）

- `dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml`：把 ZLMediaKit、可选 Pion helper、Playwright runner 组合成一份一键起 lab：
  - `zlmediakit` 默认 service：固定 tag `zlmediakit/zlmediakit:v8.0`、`network_mode: host`、`MK_OPT="-DRTC_TLS=0 -DRTC_TCP=1"`、自带 `curl /index/api/getServerConfig` 健康检查。
  - `pion-helper` profile（默认关闭，`--profile pion` 启用）：从 env 读取 WHIP/WHEP URL，依赖 ZLM healthy。
  - `playwright` profile（默认关闭，`--profile browser` 启用）：占位 runner，等具体浏览器测试镜像落地后接入；预先把 env 名定下来。
- `interop-runner.md` 新增 “docker-compose 一键起” 段，写齐 `docker compose up -d` / 选 profile / 跑 cargo test / `down` 收尾的完整命令。
- 与第三轮 nightly workflow 的关系：workflow 里仍用 `services.zlmediakit` 的 GitHub Actions 写法（受 nightly job 隔离要求约束），本地 dev / staging 用 docker-compose 复现同一组容器。

## 已完成（Phase 06 第三轮）

- `.github/workflows/webrtc-interop-nightly.yml`：定时 / 手动触发的 nightly 工作流。
  - `services.zlmediakit` 用 `--network host` 拉起 `zlmediakit/zlmediakit:master` 容器，对齐 `interop-runner.md` 的复现命令。
  - `Wait for ZLMediaKit to come up` step：60s 轮询 `/index/api/getServerConfig`，超时只输出 warning，让 ZLM 之外的测试照常 skip 而不阻塞整体 job。
  - `cargo test -p cheetah-webrtc-module --test interop -- --ignored ${FILTER:-}`：只跑 `--ignored` 集合，未配置的 env 由 harness 自动 skip。
  - `actions/upload-artifact@v4` 把 `target/webrtc-interop/` 整个上传，保留 14 天，方便事后定位。
  - `workflow_dispatch` 暴露 `run_filter` 输入字段，运营手动触发时可只跑特定 case。
- 完成后整个互操作链路是：`interop_harness` → `interop.rs` ignored 测试 → nightly workflow 跑 + 上传 artifact → `interop-runner.md` 文档手册描述如何在本地复现，闭环可观测。

## 已完成（Phase 06 第二轮）

- `dev-docs/plans-27-webrtc-zlm2/interop-runner.md`：把 `tests/interop_harness.rs` 的所有 env 约定落到具体复现命令，覆盖 ZLM WHIP/WHEP、ZLM P2P signaling、Pion、GStreamer、Janus、浏览器、跨协议（RTSP / RTMP / GB28181）、`tc netem` 弱网、CI nightly 推荐流程。
- runner 文档列出了"仍未落地"的 docker-compose / Playwright / helper 源代码缺口，确保下一轮工作的入口点清晰。
- 现有 ignored 测试都通过统一 harness skip 而不会失败，所以 `cargo test -- --ignored` 已可作为 nightly 的入口命令而不需要先逐项配置 env。

## 已完成（Phase 06 第一轮）

- `crates/protocols/webrtc/module/tests/interop_harness.rs`（同时作为 standalone test 与 `interop.rs` 的子模块）：
  - 统一环境变量常量：`ENV_ARTIFACT_DIR / ENV_TIMEOUT_MS / ENV_ZLM_BASE / ENV_ZLM_WHIP / ENV_ZLM_WHEP / ENV_ZLM_SIGNALING / ENV_BROWSER / ENV_PION_BIN / ENV_GST_BIN / ENV_JANUS / ENV_RTSP / ENV_RTMP / ENV_GB28181 / ENV_WEAK_NETWORK`。
  - `InteropArtifact::open(test_name)` 在 `target/webrtc-interop/<test>/` 下创建目录并自动写 `README.md`（含运行时 env 快照）。
  - `InteropArtifact::write / append / set_failure` 帮助方法：标准化 SDP / 日志 / failure artifact 落盘。
  - `default_artifact_root()` 自动查找最近的 `target/`，避免不同 cwd 误写。
  - `timeout()` 与 `require_env(var)` 提供统一超时和 skip 逻辑。
- `crates/protocols/webrtc/module/tests/interop.rs` 重写：
  - 全部 ignored 测试改用 `open_test(name, Some(env))` 入口，文档头部统一说明 env、artifact、复现命令。
  - 新增 `zlm_p2p_signaling_smoke`、`janus_signaling_smoke` ignored test 与对应 env 入口。
  - 失败时调用 `set_failure(...)` 写 `failure.txt`。
- `interop_harness` 自身有 5 条单元测试（artifact 创建、写入、append、env 缺失返回 None、timeout 默认值）。

## 仍未落地（下一轮）

- dual-stream simulcast 媒体面验证：simulcast offer fixture 已经验证 SDP 形态，下一步用 Playwright + Chrome `getStats()` 检查 cheetah 在三层下发后的 `outbound-rtp` 多 SSRC 的实际选层。
- 跨平台 weak-network 自动化：`tc netem` Linux 矩阵已落地（默认 `loss-5 + reorder`、manual dispatch full 6-profile）；Windows / macOS 路径仍只有文档（`interop-weak-network/WINDOWS.md`），需要 self-hosted runner + Clumsy 自动化。
- 更多 ZLM 字段差异 fixture（DTMF / INFO、SVC scalability mode、低带宽 codec switch 等）。

## 实现概览

本阶段把现有 `interop.rs` ignored scaffold 扩展为可复现的实体测试基础设施。目标是每个外部互操作失败都能留下 SDP、日志、stats 和复现命令。

## 6.1 测试分层

三层测试：

1. `unit`: 不依赖外部进程，默认 `cargo test` 运行。
2. `local-entity`: 测试启动本地 helper 或 docker container，仍默认 ignored。
3. `manual-entity`: 用户或 CI 先启动外部服务，通过环境变量注入 URL。

所有外部测试必须：

- `#[ignore]`
- env var 缺失时 skip。
- env var 存在时执行真实握手或媒体断言。
- 写 artifact 到 `target/webrtc-interop/<test-name>/`。

## 6.2 Artifact 标准

每个 test 目录包含：

```text
target/webrtc-interop/<test-name>/
  README.md
  request-offer.sdp
  response-answer.sdp
  local-candidates.txt
  remote-candidates.txt
  session-stats.json
  module-events.log
  peer.log
  failure.txt
```

如果测试没有 SDP，例如纯 room list，也要写 `session-stats.json` 和 `peer.log`。

## 6.3 Env var 约定

统一环境变量：

```text
WEBRTC_INTEROP_ARTIFACT_DIR
WEBRTC_INTEROP_TIMEOUT_MS
WEBRTC_INTEROP_ZLM_BASE_URL
WEBRTC_INTEROP_ZLM_WHIP_URL
WEBRTC_INTEROP_ZLM_WHEP_URL
WEBRTC_INTEROP_ZLM_SIGNALING_URL
WEBRTC_INTEROP_BROWSER
WEBRTC_INTEROP_PION_BIN
WEBRTC_INTEROP_GSTREAMER_BIN
WEBRTC_INTEROP_JANUS_URL
WEBRTC_INTEROP_RTSP_URL
WEBRTC_INTEROP_RTMP_URL
WEBRTC_INTEROP_GB28181_SOURCE
WEBRTC_INTEROP_WEAK_NETWORK
```

测试读取 env 后必须把实际配置写进 artifact `README.md`。

## 6.4 ZLMediaKit 互操作

测试项：

- Cheetah WHIP publish -> ZLM WHEP play。
- ZLM WHIP publish -> Cheetah WHEP play。
- Cheetah P2P client -> ZLM signaling server。
- ZLMRTCClient -> Cheetah publish/play/echo。

断言：

- HTTP status 和 SDP content-type 正确。
- ICE connected。
- first keyframe < 配置阈值。
- NACK/RTX counters 在弱网场景有变化。
- session close 后双方无泄漏。

## 6.5 Browser / ZLMRTCClient

建议使用 Playwright 或已有浏览器 runner：

- Chrome：simulcast push，验证 RID 层和 BWE 降层。
- Firefox：RID/SSRC simulcast，DataChannel echo。
- Safari：H264/Opus 基础播放，若本地环境不可用则 manual-only。

artifact：

- browser console log。
- getStats snapshot。
- offer/answer SDP。
- screenshot 可选。

## 6.6 Pion / GStreamer / Janus

Pion：

- DataChannel echo。
- WHIP/WHEP publish/play。
- NACK/RTX weak network。

GStreamer：

- `webrtcbin` publish to Cheetah。
- Cheetah WHEP play into `webrtcbin` sink。

Janus：

- SDP fixture validation。
- simulcast offer/answer。
- RTCP feedback exchange。

## 6.7 跨协议源互操作

覆盖：

- RTSP -> WebRTC。
- RTMP -> WebRTC。
- RTP/GB28181 -> WebRTC。
- WebRTC -> RTSP/RTMP/HLS/fMP4/HTTP-FLV。

断言：

- engine stream 注册成功。
- WHEP play 收到 tracks。
- 首个可解码关键帧到达。
- codec 与 `TrackInfo` 一致。
- stop 后 stream/session 清理。

## 6.8 弱网实体测试

弱网条件：

- 1%、5%、10%、20% random loss。
- burst loss。
- reorder。
- bandwidth cap。
- TCP fallback。

断言：

- NACK out > 0。
- RTX hit > 0 或 miss 有明确诊断。
- 10% loss 下可恢复首个关键帧。
- BWE 有下降和恢复事件。
- session 未异常关闭。

Linux 可用 `tc netem`，Windows 环境标记 manual-only 或使用外部网络模拟器。

## 6.9 CI 策略

默认 CI：

- 只跑 unit/integration。
- 不跑外部 ignored tests。

Nightly / manual CI：

- 启动 ZLMediaKit container。
- 启动 Pion helper。
- 启动 GStreamer helper。
- 运行 ignored interop。
- 上传 artifact。

失败处理：

- 单个外部实体失败不隐藏日志。
- flaky 测试必须标记 known issue，不允许静默 skip。
- env var 存在但实体不可用时测试失败并写 `failure.txt`。

## 6.10 测试要求

默认验证：

```powershell
cargo test -p cheetah-webrtc-module --test interop
```

外部验证：

```powershell
cargo test -p cheetah-webrtc-module --test interop -- --ignored
```

文档同步：

- 每新增一个 ignored test，都要在本文件记录实体、env、断言、artifact。
- 如果改变 HTTP API 或 P2P message schema，同步更新 `webrtc-zlm2-remaining-architecture.md`。

