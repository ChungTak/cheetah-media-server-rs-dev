# 10 · 第三方信令服务器生产 Contract

## 1. 测试分层

每类项目必须有两层：

- Rust SDK contract：调用 runtime-neutral trait 和数据面 handle。
- HTTP contract：以外部进程视角调用 native API，通过 RTP/RTSP/WHIP 等协议验证媒体。

FakeMediaProvider 只用于参数/错误注入，不计入生产完成。生产测试必须启动真实 Engine、相关 module 和 provider。

## 2. GB28181

必测流程：动态/指定 RTP 端口、UDP/TCP passive ingest、PS demux、stream online、track/frame、关键帧、播放 URL、record、RTP sender、talk、timeout 和 stop。测试不发送 SIP 消息；设备 ID/channel ID 只作为测试 metadata。

验收：发送端和接收端都观察到实际 RTP packet；录制文件可查询；关闭后端口和 publisher lease 释放。

## 3. ONVIF

测试客户端模拟 ONVIF 项目已经获得设备 Profile 和 RTSP URI：

1. 创建 RTSP pull proxy。
2. 等待目标 MediaKey online。
3. 查询 tracks 和播放 URL。
4. 请求关键帧。
5. 生成可解码快照并通过授权 handle 下载。
6. 可选开始/停止录制。
7. 删除 proxy，确认 stream/session 清理。

不测试 WS-Discovery、SOAP、PTZ 或 ONVIF 鉴权协议。

## 4. Apple HomeKit

同进程 Rust 测试通过 MediaDataPlaneApi 打开视频/音频 subscriber，验证：

- TrackInfo codec/rate/dimensions 可用于 HAP 侧选择参数。
- 关键帧请求到达 publisher。
- subscriber 接收有界 AVFrame 流。
- 慢消费不拖累其他订阅者。
- close handle 后无帧、session 注销。

外部进程测试使用 RTP sender 或受支持的实时输出。SRTP 密钥、HAP pairing 和 characteristic 不进入 Cheetah domain。

## 5. Matter

测试 capability 查询、媒体资源发现、URL 获取、subscriber、record、snapshot、event subscription 和取消。事件测试必须观察真实 RecordCompleted/SnapshotCompleted/StreamOnlineChanged，不能只断言 subscribe 返回 Ok。

Matter endpoint/cluster/commissioning 不属于测试范围。

## 6. 通用失败用例

- provider 未编译：Unavailable，capability 不宣告。
- provider 已注册但操作不支持：Unsupported。
- 第二发布者：Conflict。
- deadline：Timeout 且后台任务有确定终态。
- 重复 idempotency key：返回原资源。
- module restart：旧 handle 关闭，新 provider generation 可用。
- 未授权 principal：PermissionDenied，不泄露资源详情。

## 7. 测试文件组织

保留 `cheetah-sdk/tests/signal_contracts/`，但拆分 support：

- `fake_support`：纯错误/契约单测。
- `production_support`：真实 engine builder、module、临时端口、媒体 fixture。
- 每个项目同时包含 `_fake_contract` 与 `_production_contract`，发布门禁只认可 production 后缀。

## 8. 验收命令

```bash
cargo test -p cheetah-sdk signal_contracts
cargo test -p cheetah-sdk gb28181_production_contract
cargo test -p cheetah-sdk onvif_production_contract
cargo test -p cheetah-sdk homekit_production_contract
cargo test -p cheetah-sdk matter_production_contract
```

测试可使用 localhost socket 和临时目录，不依赖公网、真实设备或信令服务器。

