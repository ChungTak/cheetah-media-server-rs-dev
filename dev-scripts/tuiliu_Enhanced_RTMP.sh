


### 1. 推送 H.265 (HEVC) 视频

H.265 是目前最成熟的升级方案，可以在同等码率下获得比 H.264 更好的画质。

```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv  \
  -c:v libx265 -preset veryfast -b:v 4000k \
  -c:a aac -b:a 128k -channel_layout 5.1 \
  -f flv rtmp://localhost/live/test &> push.log

```

* **注意**：FFmpeg 会自动处理 Enhanced RTMP 的 FourCC 标识，不需要额外参数。

---

### 2. 推送 AV1 视频（高效率/节省带宽）

AV1 是未来的趋势，YouTube 目前已经支持通过 RTMP 接收 AV1 流。

```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv  \
  -c:v libsvtav1 -preset 8 -b:v 3000k \
  -c:a aac -b:a 128k -channel_layout 5.1 \
  -f flv rtmp://localhost/live/test &> push.log

```

* **libsvtav1**：这是目前速度较快、适合直播的 AV1 编码器。
* **preset 8**：在速度和压缩率之间取得平衡。

---

### 3. 推送带 HDR 元数据的流  -  已开发未测试，测试需要有hdr的视频文件才可以

Enhanced RTMP 的一大优势是支持 HDR 元数据传递。如果你有 HDR 源，可以保留其动态范围。

```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv  \
  -c:v libx265 -x265-params "hdr10=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc" \
  -c:a aac \
  -f flv rtmp://localhost/live/test

```

---

### 4. 推送多声道音频 (E-AC3)  -  已开发未测试，rtmp支持但librtp不支持rtp_payload_encode_input，暂时把E-AC3当作opus来传递

如果你需要推送 5.1 环绕声，可以使用增强版支持的音频格式。

```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv  \
  -c:v libx264 -b:v 5000k \
  -c:a eac3 -b:a 256k -ac 6 \
  -f flv rtmp://localhost/live/test &> push.log

```



### 5. H.266 (VVC) 推流命令 - 已开发未测试，ffmpeg 不支持

FFmpeg 8.0 深度集成了 `libvvenc`。这是目前最前沿的编码支持，虽然对服务器要求极高，但带宽节省能力惊人。

```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv \
  -c:v libvvenc -preset fast -tier main -b:v 2000k \
  -c:a aac -b:a 128k -channel_layout 5.1 \
  -f flv rtmp://localhost/live/test

```

* **注意**：VVC 的 FourCC 标识在 E-RTMP 中被定义为 `vvc1`。
* **应用场景**：极低带宽下的 8K 直播。

---

### 6. VP9 高性能推流

在 FFmpeg 8.0 中，针对 VP9 的多线程优化更好，适合在不支持 AV1 的旧硬件上实现高压缩比。

```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv \
  -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 3000k \
  -c:a opus -b:a 128k -strict -2 \
  -f flv rtmp://localhost/live/test

```

* **参数优化**：`-deadline realtime` 是直播的关键，否则 VP9 的编码速度会非常慢。

---

### 7. AV1 硬件加速推流 (NVIDIA/Intel) - 未测试

FFmpeg 8.0 对硬件加速器的 E-RTMP 封装做了更好的适配。如果你有 NVIDIA 40 系列或 Intel Arc 显卡，应优先使用显卡编码。

**NVIDIA 显卡 (NVENC):**

```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv \
  -c:v av1_nvenc -preset p4 -b:v 3000k \
  -f flv rtmp://localhost/live/test

```

**Intel 显卡 (QSV):**  - 未测试

```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv \
  -c:v av1_qsv -global_quality 25 \
  -f flv rtmp://localhost/live/test

```

---

### 8. 增强版音频格式：Opus 与 多声道 E-AC3

FFmpeg 8.0 完善了在 FLV 容器中封装非 AAC 音频的逻辑。

```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv \
  -c:v libx265 \
  -c:a libopus -mapping_family 1 -ac 6 -b:a 192k  -strict -2 \
  -f flv rtmp://localhost/live/test &> push.log
```

* **关键点**：使用 `libopus` 进行 5.1 声道推流，这在传统的 RTMP 中是不可能实现的。



### 9. 传统视频格式 (Legacy Video)

除了标准的 H.264 (AVC)，RTMP 规范中原生定义的还有以下格式：

#### **H.263 (Sorenson Spark)**

这是 Flash Player 早期最常用的视频格式，常用于低分辨率视频会议。

* **FFmpeg 命令：**
```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c:v flv1 -b:v 800k -c:a adpcm_swf -f flv rtmp://localhost/live/test

```


*注：`flv1` 是 FFmpeg 中对应的 Sorenson Spark 编码器名称。*

#### **Screen Video (V1 & V2)**

Adobe 专门为屏幕录制开发的编码，压缩文字和静态窗口效率很高，但现已被 H.264 彻底取代。

* **FFmpeg 命令：**
```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c:v flashsv -f flv rtmp://localhost/live/test

```



#### **On2 VP6**

在 H.264 流行之前，VP6 是高画质 Flash 视频的标配。

* **FFmpeg 命令：**
```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c:v vp6f -b:v 2000k -f flv rtmp://localhost/live/test

```



---

### 10. 传统音频格式 (Legacy Audio)

RTMP 规范对音频的支持非常杂，尤其是在安防领域常见的 G.711。

#### **G.711 (PCMA / PCMU)**

这是电话级别的音频编码，在老款监控摄像头的 RTMP 回传中非常常见。

* **FFmpeg 命令 (PCMA):**
```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c:v copy -c:a pcm_alaw -ar 8000 -ac 1 -f flv rtmp://localhost/live/test

```



#### **ADPCM (SWF)**

Flash 专用的自适应差分脉冲编码。

* **FFmpeg 命令：**
```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c:v copy -c:a adpcm_swf -f flv rtmp://localhost/live/test

```



#### **Nellymoser Asao**

曾经是 Flash Player 唯一支持的非专利语音编码格式。

* **FFmpeg 命令：**
```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c:v copy -c:a nellymoser -f flv rtmp://localhost/live/test

```



#### **MP3**

RTMP 原生支持 MP3，不需要任何扩展。

* **FFmpeg 命令：**
```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c:v copy -c:a libmp3lame -f flv rtmp://localhost/live/test

```

ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv \
    -c:v copy -c:a libspeex -ar 16000 -ac 1 \
    -f flv rtmp://localhost/live/test

---

### 3. RTMP 传统与增强格式完整对照表

| 类别 | 格式名称 | FFmpeg 编码器参数 | 备注 |
| --- | --- | --- | --- |
| **视频 (传统)** | H.264 (AVC) | `-c:v libx264` | 工业标准 |
|  | Sorenson Spark | `-c:v flv1` | 对应 H.263 变体 |
|  | VP6 | `-c:v vp6f` | Flash 时代的明星 |
| **视频 (增强)** | HEVC / AV1 / VP9 | `-c:v libx265 / libsvtav1` | 现代 4K/HDR 方案 |
| **音频 (传统)** | AAC | `-c:a aac` | 兼容性最好 |
|  | MP3 | `-c:a libmp3lame` | 仅支持常用采样率 |
|  | G.711 | `-c:a pcm_alaw` | 安防监控专用 |
|  | Speex | `-c:a libspeex` | 早期语音通话 |

---

### ⚠️ 特别注意事项

1. **采样率限制**：传统的 FLV/RTMP 格式（如 G.711、Nellymoser）通常对采样率有严格限制（多为 5.5kHz, 11kHz, 22kHz, 44kHz），如果推流失败，请检查 `-ar` 参数。
2. **AAC 是分水岭**：在 RTMP 中，AAC 和 H.264 使用特定的 `AudioData` 和 `VideoData` 数据包（带有 Sequence Header），而 MP3 和 H.263 则是更简单的原始封装。
