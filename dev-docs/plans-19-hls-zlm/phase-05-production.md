# Phase 05 — 生产级特性

- **状态**: 未开始
- **范围**: HLS 录制模式、VOD playlist、HTTPS/TLS、Master playlist 多码率、时间目录组织、延迟 playlist 增强
- **完成标准**: 支持 HLS 录制回放、HTTPS 安全传输、多码率自适应播放

---

## 5.1 HLS 录制模式 (seg_keep)

**ZLMediaKit 参考**: `HlsMaker::isKeep()` 为 true 时不删除旧 segment，`isLive()` 返回 false（seg_number=0 表示录制）。

**实现方案**:

```rust
// cheetah-hls-module/src/config.rs — 新增录制配置

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HlsRecordingConfig {
    /// Enable HLS recording mode (keep all segments, generate VOD playlist).
    pub enabled: bool,
    /// Maximum recording duration in seconds (0 = unlimited).
    pub max_duration_secs: u64,
    /// Maximum number of segments to keep (0 = unlimited).
    pub max_segments: usize,
    /// Output directory for recorded segments.
    pub output_dir: String,
    /// Generate VOD playlist with EXT-X-ENDLIST on stream end.
    pub generate_vod_playlist: bool,
}
```

**行为**:
- `recording.enabled=true`: 所有 segment 写入磁盘，不删除
- Playlist 包含所有 segment（不做滑动窗口）
- 流结束时追加 `#EXT-X-ENDLIST`
- 可选 `max_duration_secs` 限制录制时长（自动切割新文件）

**与 live 模式共存**:
- 同一流可同时有 live playlist（滑动窗口）和 recording playlist（全量）
- Live: `/{app}/{stream}/index.m3u8`
- Recording: `/{app}/{stream}/record.m3u8`

---

## 5.2 VOD Playlist 生成

**ZLMediaKit 参考**: `HlsMaker::makeIndexFile(eof=true)` 生成含 `#EXT-X-ENDLIST` 的完整 playlist。

**实现方案**:

```rust
// cheetah-hls-core/src/playlist.rs — 扩展

impl PlaylistBuilder {
    /// Build a VOD playlist containing all segments with EXT-X-ENDLIST.
    pub fn build_vod(
        segments: &[SegmentFileEntry],
        target_duration: u32,
        container: HlsContainer,
    ) -> String {
        let mut out = String::with_capacity(segments.len() * 64);
        out.push_str("#EXTM3U\n");
        out.push_str(&format!("#EXT-X-VERSION:{}\n", if container == HlsContainer::Fmp4 { 7 } else { 3 }));
        out.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", target_duration));
        out.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
        out.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");

        if container == HlsContainer::Fmp4 {
            out.push_str("#EXT-X-MAP:URI=\"init.mp4\"\n");
        }

        for seg in segments {
            out.push_str(&format!("#EXTINF:{:.3},\n{}\n", seg.duration_secs, seg.filename));
        }
        out.push_str("#EXT-X-ENDLIST\n");
        out
    }
}
```

---

## 5.3 HTTPS/TLS HLS 服务

**实现方案**:

在 driver 层支持 TLS listener：

```rust
// cheetah-hls-driver-tokio/src/server.rs — TLS 支持

pub struct HlsServerConfig {
    pub listen: SocketAddr,
    pub tls: Option<TlsConfig>,
}

pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

// 启动时根据配置选择 TCP 或 TLS listener
async fn start_listener(config: &HlsServerConfig) -> Listener {
    if let Some(tls) = &config.tls {
        let acceptor = load_tls_acceptor(&tls.cert_path, &tls.key_path)?;
        Listener::Tls(TlsListener::bind(&config.listen, acceptor).await?)
    } else {
        Listener::Tcp(TcpListener::bind(&config.listen).await?)
    }
}
```

**配置**:
```yaml
modules:
  hls:
    listen: 0.0.0.0:8088
    tls:
      cert_path: /etc/ssl/certs/hls.crt
      key_path: /etc/ssl/private/hls.key
```

**注意**: TLS 依赖 `tokio-rustls` 或 `tokio-native-tls`，仅在 driver 层引入。

---

## 5.4 Master Playlist 多码率

**需求**: 支持同一流的多码率变体，生成标准 Master Playlist。

**实现方案**:

```rust
// cheetah-hls-core/src/playlist.rs — Master playlist 生成

pub struct VariantInfo {
    pub bandwidth: u64,
    pub resolution: Option<(u16, u16)>,
    pub codecs: String,
    pub uri: String,
}

impl PlaylistBuilder {
    pub fn build_master(variants: &[VariantInfo]) -> String {
        let mut out = String::new();
        out.push_str("#EXTM3U\n");
        for v in variants {
            out.push_str("#EXT-X-STREAM-INF:");
            out.push_str(&format!("BANDWIDTH={}", v.bandwidth));
            if let Some((w, h)) = v.resolution {
                out.push_str(&format!(",RESOLUTION={}x{}", w, h));
            }
            out.push_str(&format!(",CODECS=\"{}\"", v.codecs));
            out.push_str(&format!("\n{}\n", v.uri));
        }
        out
    }
}
```

**多码率来源**:
- 同一流的不同转码输出（需转码模块支持，当前不在 scope）
- 同一流的不同质量推流（多路推流到不同 stream key）
- Module 层聚合多个 stream key 为一个 master playlist

**配置**:
```yaml
modules:
  hls:
    master_playlists:
      - name: "live/test"
        variants:
          - stream_key: "live/test_1080p"
            bandwidth: 5000000
            resolution: "1920x1080"
          - stream_key: "live/test_720p"
            bandwidth: 2500000
            resolution: "1280x720"
          - stream_key: "live/test_480p"
            bandwidth: 1000000
            resolution: "854x480"
```

---

## 5.5 Segment 文件名时间目录组织

**ZLMediaKit 参考**: `YYYY-MM-DD/HH/MM-SS_<index>.ts` 按时间组织目录。

**实现方案**:

```rust
// cheetah-hls-driver-tokio/src/file_writer.rs

pub enum SegmentNaming {
    /// Simple sequential: seg_0.ts, seg_1.ts, ...
    Sequential,
    /// Time-based directory: 2026-05-15/14/30-00_0.ts
    TimeBased,
}

fn segment_path(naming: &SegmentNaming, seq: u64, timestamp: SystemTime) -> PathBuf {
    match naming {
        SegmentNaming::Sequential => PathBuf::from(format!("seg_{}.ts", seq)),
        SegmentNaming::TimeBased => {
            let dt: DateTime<Local> = timestamp.into();
            PathBuf::from(format!(
                "{}/{:02}/{:02}-{:02}_{}.ts",
                dt.format("%Y-%m-%d"),
                dt.hour(),
                dt.minute(),
                dt.second(),
                seq
            ))
        }
    }
}
```

**配置**:
```yaml
modules:
  hls:
    file_output:
      segment_naming: "time_based"  # "sequential" | "time_based"
```

---

## 5.6 延迟 Playlist 增强

**现有**: 基础延迟 playlist 已实现。需增强为可配置的多级延迟。

**ZLMediaKit 参考**: `_delay.m3u8` 包含 `seg_number + segDelay` 个 segment，用于 DVR 回看。

**实现方案**:

```rust
// 扩展 SegmentRing 支持更大容量

pub struct SegmentRingConfig {
    /// Segments in live playlist.
    pub live_count: usize,
    /// Extra segments for delay playlist (DVR window).
    pub delay_count: usize,
    /// Total ring capacity = live_count + delay_count.
    pub total_capacity: usize,
}

// Playlist 生成时：
// - index.m3u8: 最新 live_count 个 segment
// - delay.m3u8: 最新 (live_count + delay_count) 个 segment
// - 两者共享同一 SegmentRing，只是 window 不同
```

**URL 路由**:
- `/{app}/{stream}/index.m3u8` → live playlist
- `/{app}/{stream}/delay.m3u8` → delay playlist (更大窗口)

**配置**:
```yaml
modules:
  hls:
    segment_count: 5
    segment_delay: 10  # delay playlist 额外保留 10 个 segment
```

---

## 5.7 流结束清理策略

**ZLMediaKit 参考**: `kDeleteDelaySec` 流结束后延迟删除。

**增强**: 流结束后的完整清理流程：

```rust
// 流结束时：
// 1. 生成最终 playlist (含 EXT-X-ENDLIST)
// 2. 如果 recording=true，保留所有文件
// 3. 如果 recording=false:
//    a. 等待 delete_delay_secs
//    b. 删除所有 segment 文件
//    c. 删除 m3u8 文件
//    d. 删除空目录
```

---

## 验证方法

1. 录制模式: 推流 30s → 停止 → 验证所有 segment 保留 + VOD playlist 含 ENDLIST
2. HTTPS: 配置 TLS → `curl https://localhost:8088/live/test.m3u8` → 验证 TLS 握手成功
3. Master playlist: 配置多码率 → 验证 master.m3u8 含正确 `#EXT-X-STREAM-INF`
4. 时间目录: 推流 → 验证磁盘目录结构为 `YYYY-MM-DD/HH/...`
5. 延迟 playlist: 验证 `delay.m3u8` 比 `index.m3u8` 多 N 个 segment
6. 流结束清理: 停止推流 → 验证 delete_delay_secs 后文件被删除
