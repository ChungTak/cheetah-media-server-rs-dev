//! HLS pull job: fetches remote HLS playlists and segments, publishes to engine.

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

/// Run a single HLS pull job with retry logic.
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

/// Minimal HTTP/1.1 GET client using RuntimeApi::connect_tcp.
async fn http_get(runtime_api: &Arc<dyn RuntimeApi>, url: &str) -> Result<Vec<u8>, String> {
    http_get_with_timeout(runtime_api, url, 10_000_000).await
}

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

fn resolve_host(host: &str, port: u16) -> Result<SocketAddr, String> {
    format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|e| format!("DNS resolve failed: {e}"))?
        .next()
        .ok_or_else(|| "no addresses resolved".to_string())
}

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
