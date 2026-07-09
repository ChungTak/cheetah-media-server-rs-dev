# Phase-04: 代理与性能 — Direct Proxy + NAT 增强 + 性能优化

## 目标

实现零解码 RTP 直接转发模式，优化 UDP NAT 穿透，提升大规模转发场景下的 CPU 效率。

## 任务清单

### 4.1 Direct Proxy 零解码 RTP 转发

**范围**：`cheetah-rtsp-module`

**参考**：ZLMediaKit `kDirectProxy`（默认开启，跳过 decode/re-encode）

**场景**：RTSP 源 → RTSP 订阅者，编解码器完全匹配时无需经过 AVFrame 中间层

**实现方案**：

1. 新增 `DirectProxySession` 模式：
   - 当源是 RTSP 推流且订阅者也是 RTSP PLAY 时
   - 且源和目标的 SDP 编解码器完全匹配
   - 跳过 depacketize → AVFrame → packetize 路径
   - 直接将 RTP 包从发布者队列转发到订阅者连接

2. 数据流路径对比：
   ```
   正常路径：RTP → depacketize → AVFrame → packetize → RTP → send
   直接代理：RTP → (seq/timestamp 重写) → send
   ```

3. RTP 头重写：
   - SSRC：替换为订阅者的 SSRC
   - Seq：维护独立的 seq 计数器（避免源 seq 泄漏）
   - Timestamp：保持不变（同 clock rate）
   - PT：如果 PT 映射不同则重写

4. 限制条件：
   - 仅 RTSP→RTSP 同协议转发
   - 不支持跨协议（RTSP→RTMP 仍需经过 AVFrame）
   - 不支持 GOP cache 对齐（新订阅者可能等待下一个关键帧）
   - 源为 TCP、目标为 UDP 且 RTP 包 > MTU 时不可用

5. 配置：
   ```yaml
   enable_direct_proxy: true
   direct_proxy:
     rewrite_ssrc: true
     rewrite_seq: true
     max_rtp_size_for_udp: 1400  # 超过此大小不走 UDP direct proxy
   ```

6. 自动降级：
   - 如果检测到不兼容条件，自动回退到正常路径
   - 降级事件记录日志

### 4.2 Scale/Speed 头处理

**范围**：`cheetah-rtsp-core` + `cheetah-rtsp-module`

**实现方案**：

1. core 层已有 Range 解析，补充 Scale/Speed 头解析：
   ```rust
   pub struct PlayParams {
       pub range: Option<RtspRange>,
       pub scale: Option<f64>,    // 播放速率（1.0=正常，2.0=2倍速）
       pub speed: Option<f64>,    // 传输速率
   }
   ```

2. module 层处理：
   - 对于 live 流：Scale/Speed 无意义，忽略并在响应中不包含
   - 对于 VOD 源（未来扩展）：传递给源控制
   - 对于代理拉流：转发 Scale/Speed 到上游服务器

3. 响应中回显实际生效的 Scale/Speed 值

### 4.3 UDP NAT 穿透增强

**范围**：`cheetah-rtsp-driver-tokio`

**参考**：ZLMediaKit 的 NAT hole-punch 检测

**实现方案**：

1. 服务端 PLAY（UDP 发送方向）：
   - SETUP 完成后不立即发送 RTP
   - 等待客户端发送 hole-punch 包（任意 UDP 包到服务端 RTP 端口）
   - 从 hole-punch 包中获取客户端真实 IP:port（可能与 SETUP 中声明的不同）
   - 超时后（可配置）使用 SETUP 中声明的地址

2. 服务端 RECORD（UDP 接收方向）：
   - 记录第一个 RTP 包的源地址
   - 如果后续包源地址变化（NAT 重绑定），自动更新

3. 客户端（Pull 任务）：
   - SETUP 完成后主动发送 hole-punch 包
   - 定期发送 keep-alive UDP 包维持 NAT 映射

4. 配置：
   ```yaml
   udp:
     nat_probe_timeout_ms: 5000
     nat_keepalive_interval_ms: 15000
     accept_source_change: true   # 允许 NAT 重绑定
   ```

### 4.4 端口池随机化分配

**范围**：`cheetah-rtsp-module` udp_ports

**参考**：ZLMediaKit `PortManager` 随机化

**问题**：顺序分配端口在服务重启后可能与旧连接冲突

**实现方案**：

1. 端口池改为随机化分配：
   ```rust
   pub struct UdpPortPool {
       range: RangeInclusive<u16>,
       allocated: HashSet<u16>,
       rng: SmallRng,
   }

   impl UdpPortPool {
       pub fn allocate_pair(&mut self) -> Option<(u16, u16)> {
           // 随机选择起始点，避免重启后冲突
           // 确保 RTP 端口为偶数，RTCP = RTP + 1
       }
   }
   ```

2. 配置：
   ```yaml
   udp:
     port_range: [30000, 35000]
     randomize_ports: true
   ```

3. 端口释放延迟：
   - 释放的端口进入冷却期（默认 30s）
   - 冷却期内不重新分配，避免旧包干扰新会话

### 4.5 性能优化措施

**范围**：跨层

| 优化项 | 描述 | 层 |
|--------|------|-----|
| RTP 包零拷贝 | Direct Proxy 模式下使用 `Bytes` 切片，不 clone payload | module |
| 批量发送 | 多个 RTP 包合并为一次 `writev` 系统调用 | driver |
| RTCP 合并 | 同一 SSRC 的 SR+SDES 合并为一个 compound packet | driver |
| 订阅者独立队列 | 慢订阅者不阻塞其他订阅者（已有，确认无退化） | module |
| 连接写缓冲 | TCP interleaved 使用 write buffer 合并小包 | driver |

## 测试计划

| 测试类型 | 内容 |
|----------|------|
| 集成测试 | Direct Proxy：源推流→订阅者收到完整 RTP 流 |
| 集成测试 | Direct Proxy 自动降级：源 TCP + 大包→订阅者 UDP→回退正常路径 |
| 集成测试 | NAT 穿透：模拟 NAT 地址变化后仍能收到 RTP |
| 性能测试 | Direct Proxy vs 正常路径 CPU 对比（100 路并发） |
| 单元测试 | 端口池随机化分配 + 冷却期 |
| 单元测试 | RTP 头重写正确性（SSRC/seq/PT） |

## 完成标准

- [ ] Direct Proxy 模式下 CPU 使用率显著低于正常路径
- [ ] NAT 环境下 UDP 传输稳定
- [ ] 端口池重启后无冲突
- [ ] Scale/Speed 头正确解析和回显
- [ ] 慢订阅者不影响其他订阅者延迟
