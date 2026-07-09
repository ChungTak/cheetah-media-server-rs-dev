# Phase 01 — HTTP 传输优化

- **状态**: 未开始
- **目标**: 对标 ABLMediaServer 的 HTTP 传输层优化，实现 Keep-Alive 长连接、大块数据分片发送、VLC/ffplay 播放器兼容、HEAD 请求支持、异步写入背压控制
- **影响 crate**: `cheetah-hls-driver-tokio`
- **参考源**: `NetServerHLS.cpp` (SendLiveHLS / SendRecordHLS)

---

## 1. HTTP Keep-Alive 长连接

### ABL 实现分析

ABLMediaServer 在 HLS 响应中始终返回：
```
Connection: keep-alive
Keep-Alive: timeout=30, max=100
```
并且**不主动关闭连接**（代码注释："服务器不能主动断开，否则VLC播放不正常, ffplay也经常播放不正常"）。

### 本地现状

当前 driver 每次响应后立即关闭 TCP 连接（`Connection: close`），导致：
- 每次 m3u8/segment 请求都需要新建 TCP 连接
- VLC/ffplay 等播放器可能因连接被关闭而中断播放
- 高并发时 TCP 连接建立开销大

### 实现方案

**driver 层 (`server.rs`) 改造：**

1. 连接循环：读取请求 → 处理 → 写响应 → 继续读取下一个请求（而非关闭）
2. 超时控制：连接空闲 30 秒后关闭（`tokio::time::timeout`）
3. 最大请求数：单连接最多处理 100 个请求后关闭
4. 响应头：`Connection: keep-alive\r\nKeep-Alive: timeout=30, max=100\r\n`
5. 客户端请求 `Connection: close` 时，响应后关闭

**关键约束：**
- 不能在 segment 发送完成前关闭连接
- 需要正确处理 `Content-Length`，客户端依赖它判断响应结束
- 解析下一个请求前需确保上一个响应完全写入

### 验收标准

- [ ] VLC 播放 HLS 流不中断（连续播放 60 秒以上）
- [ ] ffplay 播放 HLS 流不中断
- [ ] 单连接可复用多次请求（日志可观测）
- [ ] 空闲 30 秒后连接自动关闭

---

## 2. 大块数据分片发送

### ABL 实现分析

ABLMediaServer 发送 TS/fMP4 segment 时，按 128KB 分片写入网络：
```c
#define Send_TsFile_MaxPacketCount  1024*128  // 128KB

while (fFileByteCount > 0) {
    if (fFileByteCount > Send_TsFile_MaxPacketCount)
        XHNetSDK_Write(nClient, pTsFileBuffer + nPos, Send_TsFile_MaxPacketCount, nSyncWritePacket);
    else
        XHNetSDK_Write(nClient, pTsFileBuffer + nPos, fFileByteCount, nSyncWritePacket);
}
```

### 本地现状

当前 driver 一次性将整个 segment（可能 1-5MB）写入 socket，可能导致：
- 内核 TCP 发送缓冲区溢出
- 大 segment 阻塞其他连接的响应
- 内存峰值高（需要完整 segment 在内存中）

### 实现方案

**driver 层分片写入：**

```rust
const CHUNK_SIZE: usize = 128 * 1024; // 128KB

async fn write_segment(stream: &mut TcpStream, data: &[u8]) -> io::Result<()> {
    for chunk in data.chunks(CHUNK_SIZE) {
        stream.write_all(chunk).await?;
    }
    Ok(())
}
```

**配置项：**
```yaml
hls:
  send_chunk_size: 131072  # 128KB, 可调整
```

### 验收标准

- [ ] 5MB segment 分片发送，每片 128KB
- [ ] 发送过程中不阻塞其他连接
- [ ] 配置项可调整分片大小

---

## 3. VLC/ffplay 兼容性

### ABL 实现分析

关键兼容措施：
1. 不主动关闭连接（即使客户端发送 `Connection: close`）
2. HEAD 请求返回 `200 OK` + `Content-Length: 0`
3. m3u8 未就绪时返回 404（而非空 playlist）
4. segment 不存在时返回 404（而非等待）

### 实现方案

1. **不主动断开**：Keep-Alive 模式下，即使客户端请求 close，也等待客户端先断开（设置短超时 5s）
2. **HEAD 支持**：在 core 层 `HlsCore` 增加 HEAD 方法处理，返回与 GET 相同的 headers 但无 body
3. **404 语义**：m3u8 未就绪（< 3 segments）返回 404 而非空 playlist
4. **Content-Type 兼容**：TS 使用 `video/mp2t`，fMP4 使用 `video/mp4`

### 验收标准

- [ ] VLC 3.x 播放 H264 HLS 流正常
- [ ] VLC 3.x 播放 H265 HLS 流正常（fMP4 模式）
- [ ] ffplay 播放 HLS 流正常
- [ ] hls.js 浏览器播放正常

---

## 4. HEAD 请求支持

### 实现方案

**core 层 (`session.rs`)：**
- `HlsRequest` 增加 `method: HttpMethod` 字段（GET / HEAD / OPTIONS）
- HEAD 请求走与 GET 相同的路径，但 `HlsResponse` 标记 `body: None`

**driver 层 (`server.rs`)：**
- 解析请求行时识别 `HEAD` 方法
- 响应时跳过 body 写入，仅发送 headers

### 验收标准

- [ ] `curl -I http://host:port/live/test.m3u8` 返回 200 + 正确 Content-Type
- [ ] `curl -I http://host:port/live/test/seg_0.ts` 返回 200 + 正确 Content-Length

---

## 5. 异步写入与背压控制

### ABL 实现分析

ABLMediaServer 支持同步/异步两种发送模式（`nSyncWritePacket`），默认异步。异步模式下：
- 数据放入发送队列，由网络线程异步发送
- 发送失败计数超过 5 次则断开连接
- 每发送 5 个分片后 `Sleep(5)` 让出 CPU

### 实现方案

**driver 层背压机制：**

1. 使用 `tokio::io::BufWriter` 包装 `TcpStream`，设置写缓冲区 256KB
2. 写入超时：单个 chunk 写入超过 10 秒视为客户端慢，断开连接
3. 连续写入失败 3 次断开连接
4. 可选：segment 发送间 `tokio::task::yield_now()` 让出执行权

**配置项：**
```yaml
hls:
  write_timeout_secs: 10
  write_buffer_size: 262144  # 256KB
```

### 验收标准

- [ ] 慢客户端（限速 100KB/s）不阻塞其他客户端
- [ ] 写入超时后连接正确关闭
- [ ] 高并发（100 客户端同时拉流）无死锁

---

## 实现顺序

1. HEAD 请求支持（core + driver，改动最小）
2. 大块数据分片发送（driver，独立改动）
3. HTTP Keep-Alive（driver，需要重构连接循环）
4. VLC/ffplay 兼容性验证（集成测试）
5. 异步背压控制（driver，性能优化）

---

## 测试计划

```bash
# 单元测试
cargo test -p cheetah-hls-core
cargo test -p cheetah-hls-driver-tokio

# 集成测试
bash dev-scripts/check_hls_smoke.sh

# 兼容性测试（手动）
ffplay http://127.0.0.1:8088/live/test.m3u8
vlc http://127.0.0.1:8088/live/test.m3u8
curl -I http://127.0.0.1:8088/live/test.m3u8
```
