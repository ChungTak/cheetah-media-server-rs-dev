# 07 · 视频转码与 ABR

## 1. 视频发布矩阵

本期 release gate：

- 输入：H.264、H.265、MJPEG
- 输出：H.264、H.265
- 同 codec 重编码：尺寸、码率、帧率、pixel format
- 跨 codec：H.264 ↔ H.265、MJPEG → H.264/H.265
- 图片/文字 overlay 通过 avcodec image/OSD stage

VP8、VP9、AV1 可以在 preflight 中展示探测结果，但没有本期完整互操作和长稳证据前不得注册 production transcode operation。

## 2. VID-01：单流转码

`VideoTranscodeSpec` 必须包含：

- output codec、profile、bitrate、GOP、pixel format
- 可选 width/height 和 fps
- aspect policy：stretch、fit-pad 或 crop-fit
- 可选有序 overlays

未给 width/height 时保持源尺寸；只给一个维度时等比推导。所有值在获取 publisher lease 前校验，输出尺寸必须满足 codec 对齐规则。

处理链：

1. 等待 Ready TrackInfo、codec config 和随机访问 Access Unit。
2. 使用 `VideoSdk + VideoProfile` preflight 并创建高层 transcode session。
3. AVFrame 经过 `cheetah-codec` Access Unit/参数集规范化后 submit。
4. poll encoded packet，转换为 canonical AVFrame。
5. 输出 codec config、TrackInfo Ready，再发布媒体帧。
6. stop/discontinuity 时执行 flush/reset 合同。

## 3. 随机访问与参数集

- 新任务不从任意 delta frame 启动，必须等待随机访问点。
- 输出参数集由 encoder 生成并交给 `cheetah-codec` 缓存/补发；处理模块不得维护第二套 SPS/PPS/VPS parser。
- 请求关键帧、GOP、场景切换由任务策略控制；下游 PLI/FIR 通过处理服务映射为 encoder keyframe request。
- session rebuild 后第一帧必须是随机访问点并先输出最新 codec config。

## 4. ABR-01：显式梯度

`AbrLadderSpec` 包含 source 和 1–4 个 `AbrVariantSpec`：

- 独立目标 StreamKey
- output codec/profile
- width/height
- bitrate
- fps
- GOP

不自动生成默认梯度。文档示例提供 1080p/720p/480p，但所有参数仍由用户显式声明。

每档拥有独立 encoder 和 publisher，共享 source subscriber；是否共享 decode 仅在 avcodec 高层 API能保证单 worker 所有权和错误隔离时优化，不作为首版完成前置。

任一目标 StreamKey 冲突时整个 ladder 创建失败并回滚，不允许部分成功。运行中单档 encoder 失败时 Job 进入 Failed 并停止全部档位，避免 master playlist 长期引用不一致梯度。

## 5. HLS 与 WebRTC 消费

- HLS 继续使用 `HlsMasterPlaylistConfig.variants`；处理任务先使全部派生 Track Ready，HLS 才发布 master。
- variant bandwidth/resolution 必须与实际 TrackInfo/encoder spec 一致；不信任重复配置中的错误值。
- WebRTC 多层输出使用独立 H.264 派生流，按现有带宽/策略选择；本期不生成单 bitstream SVC。
- 最后消费者离开不停止显式 ladder；内部 Auto rendition 按共享任务 grace 清理。

## 6. 测试

- H.264/H.265/MJPEG 各输入到 H.264/H.265 的 required matrix。
- 输出用独立 decoder 验证帧数、尺寸、codec、随机访问点和参数集。
- 码率在固定 fixture/窗口内落入目标 ±20%，fps 和 duration 连续。
- aspect policy 使用几何 golden；overlay 与转码组合不能绕过 avcodec。
- ABR 验证四档上限、目标冲突原子回滚、单档故障全梯度清理。
- HLS master 的 BANDWIDTH/RESOLUTION 与真实派生 TrackInfo 一致，客户端能切档。
- PLI/FIR、source discontinuity、encoder reset、慢输出和 cancel 无不可解码长尾。

## 7. 完成标准

- [ ] NativeFree 和 Software 分别有 required matrix 证据，不把一个 profile 的成功外推给另一个。
- [ ] 所有输出先 Ready metadata/codec config 后媒体帧。
- [ ] ABR 不覆盖源流、不产生部分梯度、不引用未运行派生流。
- [ ] 视频 worker 和队列遵守像素率、内存和并发上限。
