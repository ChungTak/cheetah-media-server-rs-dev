# WebRTC 互操作 runner 操作手册（Phase 06）

本文档把 `crates/protocols/webrtc/module/tests/interop.rs` 与
`tests/interop_harness.rs` 的环境变量和 artifact 协议落到具体命令上，
方便在本地或 CI 上复现实体测试。

> Phase 06 状态：harness、env 约定、artifact 目录已落地（第一轮）。
> 真实实体启动脚本的最小集合已写到本文件（第二轮）。第六轮新增
> Pion helper 镜像、Playwright spec、`tc netem` 脚本骨架，路径见
> 下方 “Helper 与脚本骨架（第六轮）”。完整的真实媒体面 lab 编排
> （cheetah server 与 helper 同 compose）仍是下一轮工作。

---

## 全局约定

所有 ignored 互操作测试遵循同一份契约：

1. 缺少其专属 env var 时直接 skip，不失败。
2. 落地 artifact 到 `WEBRTC_INTEROP_ARTIFACT_DIR/<test>/`（默认
   `target/webrtc-interop/<test>/`）。
3. artifact 目录至少包含 `README.md`（运行时 env 快照）。失败时再写
   `failure.txt`。
4. 失败时不要静默 skip — 显式失败并保留 artifact。

完整 env 列表见 `tests/interop_harness.rs` 顶部常量。

---

## 复现命令（最小集合）

### ZLMediaKit WHIP / WHEP

```bash
# 1. 启动 ZLM 容器（host network，避免 NAT）
docker run --rm --net=host \
  -e MK_OPT='-DRTC_TLS=0' \
  zlmediakit/zlmediakit:master

# 2. 推流到 ZLM（任选其一）
ffmpeg -re -i sample.mp4 -c:v copy -c:a aac -f rtsp \
  rtsp://127.0.0.1:554/live/sample

# 3. 跑互操作测试
export WEBRTC_INTEROP_ZLM_WHIP_URL='http://127.0.0.1/index/api/webrtc?app=live&stream=sample&type=push'
export WEBRTC_INTEROP_ZLM_WHEP_URL='http://127.0.0.1/index/api/webrtc?app=live&stream=sample&type=play'
cargo test -p cheetah-webrtc-module --test interop -- --ignored zlm_whip_smoke
cargo test -p cheetah-webrtc-module --test interop -- --ignored zlm_p2p_signaling_smoke
```

### Pion peer

```bash
# Pion 端可以是任意支持 WHIP/WHEP 的 helper；多数项目里命名为
# `pion-whip-helper`。把可执行路径写到 env 里：
export WEBRTC_INTEROP_PION_BIN=/usr/local/bin/pion-whip-helper
export WEBRTC_INTEROP_ZLM_WHEP_URL=http://127.0.0.1:8080/whep/sample
cargo test -p cheetah-webrtc-module --test interop -- --ignored pion_pull_smoke
```

### GStreamer `webrtcbin`

```bash
export WEBRTC_INTEROP_GSTREAMER_BIN=$(command -v gst-launch-1.0)
export WEBRTC_INTEROP_ZLM_WHIP_URL=http://127.0.0.1:8080/whip/sample
cargo test -p cheetah-webrtc-module --test interop -- --ignored gstreamer_push_smoke
```

### 浏览器（Chrome / Firefox / Safari）

```bash
# 浏览器自动化由 Playwright / Selenium 接入；把 BROWSER 标记设为 1
# 表示 "我已经手动启动了浏览器"。具体的 driver 入口仍在补。
export WEBRTC_INTEROP_BROWSER=1
cargo test -p cheetah-webrtc-module --test interop -- --ignored browser_whip_whep_smoke
cargo test -p cheetah-webrtc-module --test interop -- --ignored zlmrtcclient_browser_interop
```

### 跨协议互操作

```bash
# RTSP -> WebRTC
export WEBRTC_INTEROP_RTSP_URL=rtsp://192.168.1.10:554/live
cargo test -p cheetah-webrtc-module --test interop -- --ignored cross_protocol_rtsp_to_webrtc

# RTMP -> WebRTC
export WEBRTC_INTEROP_RTMP_URL=rtmp://127.0.0.1:1935/live/sample
cargo test -p cheetah-webrtc-module --test interop -- --ignored cross_protocol_rtmp_to_webrtc

# GB28181 -> WebRTC
export WEBRTC_INTEROP_GB28181_SOURCE=tcp://gb28181-gateway:5060
cargo test -p cheetah-webrtc-module --test interop -- --ignored cross_protocol_gb28181_to_webrtc
```

### 弱网（Linux）

```bash
sudo tc qdisc add dev lo root netem loss 10% delay 50ms
export WEBRTC_INTEROP_WEAK_NETWORK=1
cargo test -p cheetah-webrtc-module --test interop -- --ignored weak_network_nack_recovery
sudo tc qdisc del dev lo root
```

---

## 一次性跑所有互操作测试

把上面 env 全部 export 后：

```bash
cargo test -p cheetah-webrtc-module --test interop -- --ignored
```

未导出的 env 对应的测试会自动 skip 而不会失败。

## docker-compose 一键起

`dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml` 把 ZLMediaKit + cheetah-server + 可选的 Pion / Playwright / GStreamer / Janus helper 容器组合成一份一键起 lab：

```bash
# 拉起 ZLM（默认 profile）
docker compose -f dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml up -d

# 可选：拉起 cheetah-server（占用 8000/UDP + 8088/TCP；调整 interop.yaml 避免与 ZLM 冲突）
docker compose -f dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml --profile cheetah up -d

# 可选：拉起 Pion helper
docker compose -f dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml --profile pion up -d

# 可选：拉起 GStreamer helper（whip / whep 双模；docker compose exec 触发）
docker compose -f dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml --profile gstreamer up -d
docker compose ... exec gstreamer-helper cheetah-gst-helper whep

# 可选：拉起 Janus helper（REST 三段式 echotest smoke）
docker compose -f dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml --profile janus up -d
docker compose ... exec janus-helper janus-smoke

# 可选：拉起 Playwright runner
docker compose -f dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml --profile browser up -d
docker compose ... exec playwright npx playwright test

# 跑互操作（同上 export env 之后）
cargo test -p cheetah-webrtc-module --test interop -- --ignored

# 收尾
docker compose -f dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml down
```

---

## Helper 与脚本骨架（第六轮）

第六轮在 `dev-docs/plans-27-webrtc-zlm2/` 下新增了三个独立可审计
的目录，把真实媒体面互操作的关键 helper 落地为可单独 `docker
build` / `npx playwright test` / `bash run-netem.sh` 的脚本：

```text
dev-docs/plans-27-webrtc-zlm2/
  interop-pion-helper/      # Pion WHIP/WHEP 双模 helper（Go）
    Dockerfile
    main.go                 # ~200 行；env WEBRTC_INTEROP_ARTIFACT_DIR
    go.mod
    README.md
  interop-playwright/       # Chrome getStats() 抓取
    whip-whep.spec.ts
    playwright.config.ts
  interop-weak-network/     # tc netem Linux 包装
    run-netem.sh            # loss-{1,5,10,20} / reorder / bw-cap profile
    README.md
  interop-gstreamer-helper/ # 真实可构建 GStreamer 镜像（第八轮）
    Dockerfile
    entrypoint.sh           # whip / whep 双模 + peer.log
    README.md
  interop-janus-helper/     # 真实可构建 Janus 镜像（第八轮）
    Dockerfile              # 派生 canyan/janus-gateway
    smoke.sh                # 三段式 echotest 握手 + JSON artifact
    README.md
  interop-cheetah-server/   # cheetah-server 真实可构建镜像（第九轮）
    Dockerfile              # workspace 根上下文 + multi-stage build
    interop.yaml            # 互操作 lab 用 config（webrtc + rtmp 默认开）
    README.md
```

`interop-docker-compose.yml` 已经把 `pion-helper` 改成 `build:`
本地 context；`playwright` service 增加 `working_dir: /work` +
specs 挂载，`docker compose --profile browser up -d` 之后 `docker
compose exec playwright npx playwright test` 即可跑骨架。第八轮新增
`gstreamer-helper` / `janus-helper` profile（`docker compose
--profile gstreamer / janus up -d` 后 exec 触发实际工作）。第九轮
新增 `cheetah-server` profile，让 cheetah 自己也以 service 形式
进入 lab，实现 ZLM ↔ cheetah ↔ Pion / GStreamer / Janus 的闭环
拓扑。

第六轮也新增 ignored 测试 `zlm_answer_sdp_validation`：当操作员把
ZLM 实际返回的 answer SDP 文件放到 `target/webrtc-interop/
zlm_answer_sdp_validation/response-answer.sdp` 后，测试会用
`assertions::assert_answer_well_formed` 验证。是 assertion helpers
与 ignored 测试体闭环的最小例子，方便在没有完整 lab 时快速做字段
差异回归。

---

## CI / nightly 推荐流程

1. `cargo test -p cheetah-webrtc-module --test interop` — 默认运行
   harness 自检（`tests::*`）。
2. nightly job 启动 ZLMediaKit 容器，导出 env，执行
   `cargo test -p cheetah-webrtc-module --test interop -- --ignored` 并
   上传 `target/webrtc-interop/` 目录作为 artifact。
3. 失败的 case 在 artifact 中保留 `failure.txt + README.md` 用于事后定位。

---

## 仍未落地

- docker-compose 模板（ZLM + cheetah + Playwright runner 一键起）。
- Playwright `*.spec.ts` 与 `getStats` 抓取脚本。
- Pion / GStreamer / Janus helper 的源代码或 binary 入口。
- 弱网 Windows 等价方案（`tc netem` 仅 Linux）。
- Nightly CI workflow 文件（`.github/workflows/`）。
