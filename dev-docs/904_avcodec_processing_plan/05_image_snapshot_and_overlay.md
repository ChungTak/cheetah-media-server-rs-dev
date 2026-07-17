# 05 · 图片、快照与水印

## 1. IMG-01：图片处理 API

`ImageProcessRequest` 使用 Cheetah 自有类型：

- source：`Encoded { payload, format }` 或 `VideoFrame { frame, track_info }`
- operations：有序 `Vec<ImageOperation>`
- output：`ImageOutput { format, jpeg_quality }`
- limits：期望最大宽高；最终仍受服务端硬上限约束

`ImageOperation` 固定覆盖：

- `Crop`
- `Resize`
- `ResizeFit`
- `Rotate90/180/270`
- `FlipHorizontal/Vertical`
- `Pad`
- `ColorSpace`
- `Blend`
- `Text`

参数必须使用具名结构。crop/目标尺寸/坐标在创建 worker 前验证；算子执行后再次验证输出尺寸和字节上限。

## 2. 输入与输出矩阵

| 输入 | NativeFree | Software | 输出 |
| --- | --- | --- | --- |
| JPEG/MJPEG | 必须 | 必须 | JPEG |
| PNG | 必须解码 | 必须解码 | JPEG |
| H.264 随机访问帧 | 必须 | 必须 | JPEG |
| H.265 随机访问帧 | 必须 | 必须 | JPEG |
| 非随机访问视频帧 | 不单独解码 | 不单独解码 | `InvalidArgument` |
| 任意输入 → PNG | 不支持 | 不支持 | `Unsupported` |

快照等待关键帧时保留现有 timeout/cancel 语义。收到 sequence header/codec config 后继续等待首个随机访问 Access Unit，不把配置包当作图片。

## 3. AVFrame 到图片

1. 根据 `TrackInfo` 和 frame format 生成 decoder request。
2. 输入前通过 `cheetah-codec` 得到完整 Access Unit 和参数集视图。
3. avcodec decode 得到内部 Image。
4. 顺序执行 image operations；等比最大尺寸映射到 resize-pad/fit。
5. 使用 JPEG encoder 输出 payload。
6. 独立解析 JPEG header 得到最终 width/height，不相信请求值。
7. 返回 `ImageArtifact`；Snapshot module 再按现有原子文件提交和 FileHandle 生命周期持久化。

Snapshot module 不再有私有 image fallback，也不直接依赖 avcodec-rs。

## 4. 水印资产

- `ImageOverlay` 引用授权 `FileHandle`，支持 JPEG/PNG logo、anchor、margin、global alpha。
- `TextOverlay` 引用授权字体 `FileHandle`，支持 UTF-8 text、anchor、margin、size、color、alpha。
- 不接受任意绝对/相对文件路径、URL、base64 YAML 或运行时字体发现。
- module config 可以指定一个受控默认字体；请求文字但无字体时返回 `InvalidArgument`。
- 任务更新携带 `expected_generation`，新 overlay 先完整加载/preflight，再在下一随机访问点原子替换；失败保留旧 generation。
- logo/font 解码结果有总字节和像素上限，按内容 hash 缓存，缓存有条目数与字节上限。

## 5. Native HTTP

`POST /api/v1/images/process`：

- multipart 中包含 image/frame payload 和 JSON operation spec；
- adapter 限制 body、part 数、文件名长度和 content type；
- 成功返回 JPEG bytes、`image/jpeg`、最终尺寸 headers；
- PNG 输出请求稳定映射 `Unsupported`；
- Domain provider 仍只接收 bytes/FileHandle，不接收 multipart 类型。

## 6. 测试

- 每个算子使用小型确定性 fixture 做像素或感知 hash golden。
- JPEG/PNG 输入经 crop/resize/rotate/flip/pad/CSC 后输出可由独立 decoder 读取。
- H.264/H.265/MJPEG 关键帧快照覆盖横竖分辨率、奇数尺寸和缺参数集。
- 图片+文字水印验证锚点、透明度、CJK glyph、generation 原子更新。
- corrupt/truncated/bomb/超大尺寸、非法 crop、越界 blend、无 glyph 字体返回稳定错误且无资源泄漏。
- Snapshot 文件原子提交、查询、删除和事件合同继续通过。
- `cargo tree` 证明 snapshot/image crate 不直接依赖 `image` 或 avcodec backend。

## 7. 完成标准

- [ ] Snapshot capability 只有 ImageProcessing/JPEG preflight 通过时才声明 capture operation。
- [ ] PNG enum 可解析但不被能力报告承诺，调用不产生空文件或伪成功。
- [ ] 所有解码、算子和 JPEG 编码均通过 avcodec 高层 session。
- [ ] 图片处理在 blocking worker 上完成，取消/超时后内存和任务回到基线。
