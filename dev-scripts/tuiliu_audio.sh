
### 1. AAC Only
RTSP 传输 AAC 通常需要设置为 ADTS 封装，且采样率建议保持在 44100Hz 或 48000Hz。
```bash
ffmpeg -stream_loop -1 -re -i ./test_media_files/gst_generated/fallback_audio.wav \
-c:a aac -b:a 128k -ar 44100 -ac 2 \
-f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/live/test
```

### 2. MP3 Only
```bash
ffmpeg -stream_loop -1 -re -i ./test_media_files/gst_generated/fallback_audio.wav \
-c:a libmp3lame -b:a 128k -ar 44100 -ac 2 \
-f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/live/test
```

### 3. Opus Only
Opus 在 RTSP 中通常映射为 RTP 负载，注意采样率强制为 48000Hz 是 Opus 的标准。
```bash
ffmpeg -stream_loop -1 -re -i ./test_media_files/gst_generated/fallback_audio.wav \
-c:a libopus -b:a 64k -ar 48000 -ac 2 \
-f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/live/test
```

### 4. G.711a (PCMA) Only
常用于安防监控，采样率固定为 **8000Hz**，单声道。
```bash
ffmpeg -stream_loop -1 -re -i ./test_media_files/gst_generated/fallback_audio.wav \
-c:a pcm_alaw -ar 8000 -ac 1 \
-f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/live/test
```

### 5. G.711u (PCMU) Only
采样率固定为 **8000Hz**，单声道。
```bash
ffmpeg -stream_loop -1 -re -i ./test_media_files/gst_generated/fallback_audio.wav \
-c:a pcm_mulaw -ar 8000 -ac 1 \
-f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/live/test
```

### 6. ADPCM (DVI4/IMA) Only
RTSP 中常用的 ADPCM 变体是 `adpcm_ima_wav` 或针对 RTP 的 `adpcm_g726`。这里推荐通用的 IMA ADPCM：
```bash
ffmpeg -stream_loop -1 -re -i ./test_media_files/gst_generated/fallback_audio.wav \
-c:a adpcm_ima_wav -ar 8000 -ac 1 \
-f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/live/test
```

---

### 关键参数说明：
*   **`-c:a`**: 指定音频编码器（Codec Audio）。
*   **`-ar`**: 设置采样率（Audio Rate）。对于 G.711 等电信级编码，必须设为 8000。
*   **`-ac`**: 设置声道数（Audio Channels）。G.711 通常仅支持 1（单声道）。
*   **`-b:a`**: 设置音频比特率，AAC 和 MP3 建议设置，G.711 是固定比特率不需要设置。
*   **`-f rtsp`**: 强制输出格式为 RTSP。

**注意：** 某些 RTSP 服务器（如 `mediamtx` 或 `rtsp-simple-server`）对编码格式有严格要求。如果推流失败，请检查服务器日志是否支持该 Payload 类型。
