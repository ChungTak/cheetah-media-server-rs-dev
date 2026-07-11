# 05 · R3：能力矩阵与实现一致（`supports` 诚实）

> **Agent 用途**：阶段 1 主文档——消灭 `supports()==true` 但 `open_*` 恒 `UnsupportedProtocol`。  
> **可与 R1/R2 并行策略**：接线前 `supports=false`；接线后 `true`。

---

## 1. 目标 / 非目标

**目标**：外部 integrator 可用 `supports(protocol, direction)` 做可靠能力判定。

**非目标**：扩展 capability matrix 到 HLS/TS 等（后置）。

---

## 2. 现状

| 对 | supports | open 实际 |
| --- | --- | --- |
| RTSP pull | true | UnsupportedProtocol |
| HTTP-FLV pull | true | 可用 |
| RTMP push | true | 可用 |
| WebRTC push | true | UnsupportedProtocol |
| 非法方向 | false | UnsupportedProtocol |

`tests/capability_matrix.rs` 若断言“supports true 但 open 失败可接受”，**必须删除/改写**。

---

## 3. 契约（钉死）

```text
R3-C1: supports(p,d) == true
  ⇒ feature 启用且 adapter 已接线
  ⇒ open_*(p, …) 对「形态合法的 URL + 默认 options」不会仅因「未实现」返回 UnsupportedProtocol
  （仍可因网络/对端/坏 URL 失败）

R3-C2: supports(p,d) == false
  ⇒ open_* 返回 UnsupportedProtocol 或 FeatureDisabled（文档二选一并测）

R3-C3: 非法方向恒 supports false 且 open 拒绝
```

### 3.1 实现策略（二选一，S0 钉死）

| 策略 | 时机 | 说明 |
| --- | --- | --- |
| **A. 先诚实后接线（推荐）** | S1 立即 | RTSP/WebRTC supports→false；S2/S3 接线后改 true |
| **B. 先接线后诚实** | S2/S3 同期 | 保持 true，但同一 PR 必须完成 adapter |

**禁止**：长期 true + UnsupportedProtocol。

### 3.2 proposed 代码形态

```rust
// protocol.rs
pub fn supports(protocol: Protocol, direction: Direction) -> bool {
    match (protocol, direction) {
        (Protocol::HttpFlv, Direction::Pull) => cfg!(feature = "http-flv"),
        (Protocol::Rtmp, Direction::Push) => cfg!(feature = "rtmp"),
        (Protocol::Rtsp, Direction::Pull) => {
            cfg!(feature = "rtsp") && cfg!(feature = "rtsp_pull_wired")
            // 或简单：cfg!(feature = "rtsp") 且编译期常量 RTSP_PULL_WIRED
        }
        (Protocol::WebRtc, Direction::Push) => {
            cfg!(feature = "webrtc") && WEBRTC_PUSH_WIRED
        }
        _ => false,
    }
}

// 更简单（推荐实现 agent 采用）：
// 用源码内 const：
const RTSP_PULL_IMPLEMENTED: bool = true; // 接线后改 true；接线前 false
```

不必新增 Cargo feature `rtsp_pull_wired`，除非想更细；**const 或直接改 match 即可**。

---

## 4. 测试清单

| ID | 用例 | 期望 |
| --- | --- | --- |
| T-CAP-01 | 四合法对 supports 与 feature | 表驱动 |
| T-CAP-02 | 非法方向 supports false + open 错 | 一致 |
| T-CAP-03 | supports true 时 open 不因 Unimplemented 失败 | 用合法形态 URL mock/loopback |
| T-CAP-04 | supports false 时 open 明确拒绝 | Unsupported 或 FeatureDisabled |
| T-CAP-05 | rustdoc 矩阵与代码一致 | 人工/字符串测 |

T-CAP-03 在 RTSP/WebRTC 未接线时：若采用策略 A，supports 为 false，本条对该对跳过。

---

## 5. DoD（阶段 1）

- [ ] 无“supports true + 立即 UnsupportedProtocol（未实现）”组合  
- [ ] capability_matrix 测与契约一致  
- [ ] README/rustdoc 矩阵更新  
- [ ] 与 R1/R2 合并时 supports 翻转有测试锁住  

---

## 6. 衔接

- R1/R2 完成时 **必须** 把对应 supports 置 true 并加 T-CAP-03。  
- example `external_connector_loopback` 打印矩阵应反映真实能力。  
