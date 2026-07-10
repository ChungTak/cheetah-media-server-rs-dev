//! HLS pull job: fetches remote HLS playlists and segments, publishes to engine.
//!
//! Implements a runtime-neutral HTTP/1.1 client on top of `RuntimeApi::connect_tcp`
//! and a TS demuxer to ingest remote HLS streams into the engine.
//!
//! HLS 拉流任务：获取远程 HLS 播放列表和分段，并发布到引擎。
//!
//! 基于 `RuntimeApi::connect_tcp` 实现运行时无关的 HTTP/1.1 客户端，并通过 TS 解复用器
//! 将远程 HLS 流接入引擎。
//!

use std::collections::HashSet;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use cheetah_codec::MonoTime;
use cheetah_hls_core::parser::parse_media_playlist;
use cheetah_hls_core::TsDemuxer;
use cheetah_sdk::{CancellationToken, RuntimeApi};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::{debug, warn};

use crate::config::HlsPullJobConfig;

/// Run a single HLS pull job with retry and exponential backoff.
///
/// Loops until cancelled, fetching the remote playlist, downloading new segments,
/// and demuxing them. Backoff grows up to `max_retry_backoff_ms` on errors.
///
/// 以重试和指数退避运行单个 HLS 拉流任务。
///
/// 循环直到被取消，获取远程播放列表、下载新分段并解复用。出错时退避逐步增长至
/// `max_retry_backoff_ms`。
pub async fn run_hls_pull_job(
    runtime_api: Arc<dyn RuntimeApi>,
    config: HlsPullJobConfig,
    cancel: CancellationToken,
) {
    debug!(
        "HLS pull job '{}' started: {} -> {}",
        config.name, config.source_url, config.target_stream_key
    );

    let mut backoff_us = config.retry_backoff_ms * 1000;
    let max_backoff_us = config.max_retry_backoff_ms * 1000;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        match run_pull_session(&runtime_api, &config, &cancel).await {
            Ok(()) => {
                // Clean exit (cancelled)
                break;
            }
            Err(e) => {
                warn!(
                    "HLS pull job '{}' error: {e}, retrying in {}ms",
                    config.name,
                    backoff_us / 1000
                );
            }
        }

        // Wait with backoff
        let deadline = MonoTime::from_micros(runtime_api.now().as_micros() + backoff_us);
        let mut timer = runtime_api.sleep_until(deadline);
        let cancelled = cancel.cancelled().fuse();
        let wait = timer.wait().fuse();
        pin_mut!(cancelled, wait);
        select_biased! {
            _ = cancelled => break,
            _ = wait => {}
        }

        backoff_us = (backoff_us * 2).min(max_backoff_us);
    }

    debug!("HLS pull job '{}' stopped", config.name);
}

/// Execute one pull session: poll m3u8, download segments, demux, repeat.
///
/// Tracks already-downloaded URIs in a bounded set and trims it when the list grows
/// beyond 100 entries to bound memory.
///
/// 执行一次拉流会话：轮询 m3u8、下载分段、解复用、循环。
///
/// 在一个有界的集合中记录已下载 URI，并在列表超过 100 条时裁剪，以限制内存。
async fn run_pull_session(
    runtime_api: &Arc<dyn RuntimeApi>,
    config: &HlsPullJobConfig,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let mut downloaded: HashSet<String> = HashSet::new();
    let mut demuxer = TsDemuxer::new();
    let playlist_interval_us: u64 = 2_000_000; // 2 seconds between m3u8 requests

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }

        // Fetch m3u8
        let playlist_body = http_get(runtime_api, &config.source_url).await?;
        let playlist_text =
            std::str::from_utf8(&playlist_body).map_err(|e| format!("m3u8 not utf8: {e}"))?;

        // Parse media playlist
        let parsed =
            parse_media_playlist(playlist_text).map_err(|e| format!("m3u8 parse error: {e:?}"))?;

        // Download new segments
        for seg in &parsed.segments {
            if cancel.is_cancelled() {
                return Ok(());
            }
            if downloaded.contains(&seg.uri) {
                continue;
            }

            // Resolve segment URL (relative to m3u8 base)
            let seg_url = resolve_url(&config.source_url, &seg.uri);

            // Download segment with timeout
            match http_get_with_timeout(runtime_api, &seg_url, 6_000_000).await {
                Ok(seg_data) => {
                    downloaded.insert(seg.uri.clone());
                    // Demux TS segment — frames produced for future publish integration
                    // TODO: publish demuxed frames to engine via PublisherApi
                    let _events = demuxer.feed_segment(&seg_data);
                }
                Err(e) => {
                    warn!("HLS pull '{}': segment download failed: {e}", config.name);
                    break;
                }
            }
        }

        // Trim downloaded set (keep only recent entries to bound memory)
        if downloaded.len() > 100 {
            downloaded.clear();
            for seg in &parsed.segments {
                downloaded.insert(seg.uri.clone());
            }
        }

        // Wait before next m3u8 request
        let deadline = MonoTime::from_micros(runtime_api.now().as_micros() + playlist_interval_us);
        let mut timer = runtime_api.sleep_until(deadline);
        let cancelled = cancel.cancelled().fuse();
        let wait = timer.wait().fuse();
        pin_mut!(cancelled, wait);
        select_biased! {
            _ = cancelled => return Ok(()),
            _ = wait => {}
        }
    }
}

/// Minimal HTTP/1.1 GET with the default 10 s timeout.
///
/// 使用默认 10 秒超时的最小 HTTP/1.1 GET。
async fn http_get(runtime_api: &Arc<dyn RuntimeApi>, url: &str) -> Result<Vec<u8>, String> {
    http_get_with_timeout(runtime_api, url, 10_000_000).await
}

/// HTTP/1.1 GET with a per-read timeout.
///
/// Connects over TCP, sends a request, and reads the response with a timeout on
/// each read. Returns the body after the header terminator.
///
/// 带每次读取超时的 HTTP/1.1 GET。
///
/// 通过 TCP 连接、发送请求并在每次读取上设置超时，返回头部结束符后的响应体。
async fn http_get_with_timeout(
    runtime_api: &Arc<dyn RuntimeApi>,
    url: &str,
    timeout_us: u64,
) -> Result<Vec<u8>, String> {
    let (host, port, path) = parse_http_url(url)?;

    let addr = resolve_host(&host, port)?;
    let mut stream = runtime_api
        .connect_tcp(addr)
        .map_err(|e| format!("connect failed: {e}"))?;

    // Send HTTP request
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: cheetah-hls/1.0\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write failed: {e}"))?;

    // Read response with per-read timeout via runtime-neutral timer.
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut chunk = vec![0u8; 32 * 1024];

    loop {
        let read_result = {
            let deadline = MonoTime::from_micros(runtime_api.now().as_micros() + timeout_us);
            let mut timer = runtime_api.sleep_until(deadline);
            let read = stream.read(&mut chunk).fuse();
            let timeout = timer.wait().fuse();
            pin_mut!(read, timeout);
            select_biased! {
                r = read => r,
                _ = timeout => return Err("timeout".to_string()),
            }
        };
        match read_result {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > 50 * 1024 * 1024 {
                    return Err("response too large".to_string());
                }
            }
            Err(e) => return Err(format!("read failed: {e}")),
        }
    }

    // Parse HTTP response: find body after \r\n\r\n
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no header end")?;

    // Check status code
    let header = std::str::from_utf8(&buf[..header_end]).map_err(|_| "header not utf8")?;
    let status_line = header.lines().next().ok_or("empty response")?;
    if !status_line.contains("200") {
        return Err(format!("HTTP error: {status_line}"));
    }

    Ok(buf[header_end + 4..].to_vec())
}

/// Parse an `http://` URL into host, port, and path.
///
/// 将 `http://` URL 解析为主机、端口和路径。
fn parse_http_url(url: &str) -> Result<(String, u16, String), String> {
    let url = url
        .strip_prefix("http://")
        .ok_or("only http:// supported")?;
    let (host_port, path) = match url.find('/') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, "/"),
    };
    let (host, port) = match host_port.find(':') {
        Some(i) => (
            &host_port[..i],
            host_port[i + 1..]
                .parse::<u16>()
                .map_err(|e| e.to_string())?,
        ),
        None => (host_port, 80),
    };
    Ok((host.to_string(), port, path.to_string()))
}

/// Resolve host:port into a `SocketAddr`.
///
/// 将 host:port 解析为 `SocketAddr`。
fn resolve_host(host: &str, port: u16) -> Result<SocketAddr, String> {
    format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|e| format!("DNS resolve failed: {e}"))?
        .next()
        .ok_or_else(|| "no addresses resolved".to_string())
}

/// Resolve a possibly relative URI against an m3u8 base URL.
///
/// 将可能是相对的 URI 根据 m3u8 基础 URL 解析为完整 URL。
fn resolve_url(base_url: &str, relative: &str) -> String {
    if relative.starts_with("http://") || relative.starts_with("https://") {
        return relative.to_string();
    }
    // Get base path (everything up to last '/')
    if let Some(last_slash) = base_url.rfind('/') {
        format!("{}/{}", &base_url[..last_slash], relative)
    } else {
        relative.to_string()
    }
}
