//! Streaming HTTP/WS-FLV subscriber that implements `cheetah_sdk::SubscriberSource`.
//!
//! Unlike `pull_http_flv_once`, this keeps the TCP connection open and emits
//! `AVFrame` values as they are demuxed from the live FLV stream.
//!
//! 流式 HTTP/WS-FLV 订阅者，实现 `cheetah_sdk::SubscriberSource`。
//!
//! 与 `pull_http_flv_once` 不同，此实现保持 TCP 连接打开，并在实时 FLV 流
//! 解复用时逐步发出 `AVFrame`。

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use parking_lot::Mutex;

use async_trait::async_trait;
use cheetah_codec::{AVFrame, FlvDemuxer, FlvIngress, FlvIngressOutput, MonoTime, TrackInfo};
use cheetah_runtime_api::{CancellationToken, JoinHandle, RuntimeApi};
use cheetah_sdk::{SdkError, SubscriberId, SubscriberSource};
use futures::channel::mpsc;
use futures::{pin_mut, select_biased, FutureExt, SinkExt, StreamExt};

use super::{
    connect_stream, read_http_response_head, response_is_chunked, select_read_or_cancel,
    try_decode_chunked_body_frame, ChunkedBodyFrame, HttpFlvPullError, ParsedPullUrl,
    PullReadLimits,
};

/// Reconnect/backoff policy for a streaming HTTP/WS-FLV subscriber.
///
/// HTTP/WS-FLV 流订阅者的重连/退避策略。
#[derive(Debug, Clone, Copy)]
pub struct ReconnectPolicy {
    /// Maximum number of reconnection attempts.
    ///
    /// 最大重连尝试次数。
    pub max_attempts: u32,
    /// Initial backoff duration in milliseconds.
    ///
    /// 初始退避时长（毫秒）。
    pub initial_backoff_ms: u64,
    /// Maximum backoff duration in milliseconds.
    ///
    /// 最大退避时长（毫秒）。
    pub max_backoff_ms: u64,
    /// Backoff multiplier expressed in thousandths (e.g. 2000 = 2.0x).
    ///
    /// 退避乘数，以千分之一表示（例如 2000 = 2.0x）。
    pub multiplier_x1000: u32,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 5_000,
            multiplier_x1000: 2_000,
        }
    }
}

/// Options for opening a streaming HTTP/WS-FLV subscriber.
///
/// 打开流式 HTTP/WS-FLV 订阅者的选项。
#[derive(Debug, Clone)]
pub struct HttpFlvSubscriberOptions {
    /// Read limits and buffer sizes.
    ///
    /// 读取限制与缓冲区大小。
    pub read_limits: PullReadLimits,
    /// Reconnect policy. `None` disables reconnection.
    ///
    /// 重连策略。`None` 表示禁用重连。
    pub reconnect: Option<ReconnectPolicy>,
    /// Channel buffer size for emitted frames.
    ///
    /// 发出帧的通道缓冲区大小。
    pub buffer_size: usize,
    /// Optional cancellation token. The subscriber creates a child token from it.
    ///
    /// 可选取消令牌。订阅者会从中创建子令牌。
    pub cancel: Option<CancellationToken>,
    /// Pre-resolved peer address. When `Some`, the subscriber connects to this
    /// address instead of re-resolving the URL hostname, which prevents DNS rebinding.
    pub peer: Option<SocketAddr>,
}

impl Default for HttpFlvSubscriberOptions {
    fn default() -> Self {
        Self {
            read_limits: PullReadLimits::default(),
            reconnect: None,
            buffer_size: 64,
            cancel: None,
            peer: None,
        }
    }
}

/// Streaming HTTP/WS-FLV subscriber handle.
///
/// 流式 HTTP/WS-FLV 订阅者句柄。
pub struct HttpFlvSubscriber {
    id: SubscriberId,
    rx: mpsc::Receiver<Result<Arc<AVFrame>, SdkError>>,
    cancel: CancellationToken,
    join: Option<Box<dyn JoinHandle>>,
    ingress: Arc<Mutex<FlvIngress>>,
}

impl fmt::Debug for HttpFlvSubscriber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpFlvSubscriber")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl HttpFlvSubscriber {
    /// Return the subscriber id.
    ///
    /// 返回订阅者 id。
    pub fn id(&self) -> SubscriberId {
        self.id
    }

    /// Snapshot of the tracks discovered so far.
    ///
    /// 当前已发现的轨道快照。
    pub fn tracks(&self) -> Vec<TrackInfo> {
        let guard = self.ingress.lock();
        guard.tracks().to_vec()
    }
}

#[async_trait]
impl SubscriberSource for HttpFlvSubscriber {
    async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, SdkError> {
        match self.rx.next().await {
            Some(Ok(frame)) => Ok(Some(frame)),
            Some(Err(err)) => Err(err),
            None => Ok(None),
        }
    }

    async fn close(&mut self) -> Result<(), SdkError> {
        self.cancel.cancel();
        self.rx.close();
        if let Some(join) = self.join.take() {
            let _ = join.wait().await;
        }
        Ok(())
    }

    fn id(&self) -> SubscriberId {
        self.id
    }

    fn tracks(&self) -> Vec<TrackInfo> {
        // Call the inherent `tracks` method on `HttpFlvSubscriber`.
        let guard = self.ingress.lock();
        guard.tracks().to_vec()
    }
}

impl Drop for HttpFlvSubscriber {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

static NEXT_SUBSCRIBER_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Open a streaming HTTP/WS-FLV subscriber for `source_url`.
///
/// 为 `source_url` 打开一个流式 HTTP/WS-FLV 订阅者。
pub async fn open_http_flv_subscriber(
    runtime_api: Arc<dyn RuntimeApi>,
    source_url: &str,
    mut options: HttpFlvSubscriberOptions,
) -> Result<HttpFlvSubscriber, HttpFlvPullError> {
    let mut parsed = ParsedPullUrl::parse(source_url)?;
    if options.peer.is_some() {
        parsed.peer = options.peer.take();
    }
    let (tx, rx) = mpsc::channel(options.buffer_size);
    let parent_cancel = options.cancel.clone().unwrap_or_default();
    let cancel = parent_cancel.child_token();
    let task_cancel = cancel.child_token();

    let id = SubscriberId(NEXT_SUBSCRIBER_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let ingress = Arc::new(Mutex::new(FlvIngress::new()));
    let ingress_for_task = ingress.clone();
    let spawn_runtime = runtime_api.clone();
    let join = spawn_runtime.spawn(Box::pin(streaming_pull_task(
        runtime_api,
        parsed,
        task_cancel,
        options,
        ingress_for_task,
        tx,
    )));

    Ok(HttpFlvSubscriber {
        id,
        rx,
        cancel,
        join: Some(join),
        ingress,
    })
}

async fn streaming_pull_task(
    runtime_api: Arc<dyn RuntimeApi>,
    parsed: ParsedPullUrl,
    cancel: CancellationToken,
    options: HttpFlvSubscriberOptions,
    ingress: Arc<Mutex<FlvIngress>>,
    mut tx: mpsc::Sender<Result<Arc<AVFrame>, SdkError>>,
) {
    let limits = options.read_limits;
    if let Some(policy) = options.reconnect {
        let mut backoff = policy.initial_backoff_ms;
        let mut attempt = 0u32;
        loop {
            if cancel.is_cancelled() {
                break;
            }
            attempt = attempt.saturating_add(1);
            match streaming_pull_once(
                runtime_api.clone(),
                &parsed,
                &cancel,
                limits,
                &ingress,
                &mut tx,
            )
            .await
            {
                Ok(()) => break,
                Err(err) if err.retryable() && attempt < policy.max_attempts => {
                    wait_backoff(runtime_api.clone(), &cancel, backoff).await;
                    backoff = (backoff * u64::from(policy.multiplier_x1000) / 1000)
                        .min(policy.max_backoff_ms);
                    if backoff == 0 {
                        backoff = 1;
                    }
                }
                Err(err) => {
                    let _ = tx.send(Err(map_http_flv_pull_error(err))).await;
                    break;
                }
            }
        }
    } else if let Err(err) =
        streaming_pull_once(runtime_api, &parsed, &cancel, limits, &ingress, &mut tx).await
    {
        let _ = tx.send(Err(map_http_flv_pull_error(err))).await;
    }
}

async fn streaming_pull_once(
    runtime_api: Arc<dyn RuntimeApi>,
    parsed: &ParsedPullUrl,
    cancel: &CancellationToken,
    limits: PullReadLimits,
    ingress: &Arc<Mutex<FlvIngress>>,
    tx: &mut mpsc::Sender<Result<Arc<AVFrame>, SdkError>>,
) -> Result<(), HttpFlvPullError> {
    ingress.lock().reset();

    let mut stream = connect_stream(runtime_api.clone(), parsed)?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: video/x-flv\r\n\r\n",
        parsed.path_and_query, parsed.authority
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| HttpFlvPullError::WriteRequest(err.to_string()))?;

    let (status, headers, body_prefix) =
        read_http_response_head(&mut stream, cancel, limits).await?;
    if !(200..=299).contains(&status) {
        return Err(HttpFlvPullError::BadStatusCode {
            status_code: status,
        });
    }

    let mut demuxer = FlvDemuxer::new(limits.max_demux_buffer_bytes.max(1024));

    if response_is_chunked(&headers) {
        stream_chunked(
            &mut stream,
            body_prefix,
            cancel,
            limits,
            &mut demuxer,
            ingress,
            tx,
        )
        .await
    } else {
        stream_content_length(
            &mut stream,
            body_prefix,
            cancel,
            limits,
            &mut demuxer,
            ingress,
            tx,
        )
        .await
    }
}

async fn stream_content_length(
    stream: &mut Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    body_prefix: Vec<u8>,
    cancel: &CancellationToken,
    limits: PullReadLimits,
    demuxer: &mut FlvDemuxer,
    ingress: &Arc<Mutex<FlvIngress>>,
    tx: &mut mpsc::Sender<Result<Arc<AVFrame>, SdkError>>,
) -> Result<(), HttpFlvPullError> {
    if !body_prefix.is_empty() {
        push_demux_events(demuxer, ingress, &body_prefix, tx).await?;
    }

    let mut chunk = vec![0u8; limits.read_buffer_size.max(1024)];
    loop {
        let n = select_read_or_cancel(cancel, stream.read(&mut chunk)).await?;
        if n == 0 {
            break;
        }
        push_demux_events(demuxer, ingress, &chunk[..n], tx).await?;
    }
    Ok(())
}

async fn stream_chunked(
    stream: &mut Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    mut buffered: Vec<u8>,
    cancel: &CancellationToken,
    limits: PullReadLimits,
    demuxer: &mut FlvDemuxer,
    ingress: &Arc<Mutex<FlvIngress>>,
    tx: &mut mpsc::Sender<Result<Arc<AVFrame>, SdkError>>,
) -> Result<(), HttpFlvPullError> {
    let mut chunk = vec![0u8; limits.read_buffer_size.max(1024)];
    let max_chunk_bytes = limits.max_demux_buffer_bytes.max(1024);

    loop {
        while let Some(frame) = try_decode_chunked_body_frame(&mut buffered, max_chunk_bytes)? {
            match frame {
                ChunkedBodyFrame::Data(payload) => {
                    push_demux_events(demuxer, ingress, &payload, tx).await?;
                }
                ChunkedBodyFrame::End => return Ok(()),
            }
        }

        let n = select_read_or_cancel(cancel, stream.read(&mut chunk)).await?;
        if n == 0 {
            return Err(HttpFlvPullError::InvalidChunkedEncoding(
                "unexpected EOF before terminating chunk".to_string(),
            ));
        }
        buffered.extend_from_slice(&chunk[..n]);
        if buffered.len() > max_chunk_bytes {
            return Err(HttpFlvPullError::InvalidChunkedEncoding(format!(
                "chunked decoder buffer too large: {} > {}",
                buffered.len(),
                max_chunk_bytes
            )));
        }
    }
}

async fn push_demux_events(
    demuxer: &mut FlvDemuxer,
    ingress: &Arc<Mutex<FlvIngress>>,
    bytes: &[u8],
    tx: &mut mpsc::Sender<Result<Arc<AVFrame>, SdkError>>,
) -> Result<(), HttpFlvPullError> {
    let events = demuxer.push(bytes).map_err(HttpFlvPullError::FlvDemux)?;
    for event in events {
        let output = {
            let mut guard = ingress.lock();
            guard
                .process_event(event)
                .map_err(|err| HttpFlvPullError::Ingress(err.to_string()))?
        };
        if let Some(output) = output {
            match output {
                FlvIngressOutput::Track(_tracks) => {
                    // Track updates are surfaced when the caller queries `tracks()`.
                }
                FlvIngressOutput::Frame(frame) => {
                    tx.send(Ok(Arc::new(*frame)))
                        .await
                        .map_err(|_| HttpFlvPullError::Cancelled)?;
                }
            }
        }
    }
    Ok(())
}

async fn wait_backoff(runtime_api: Arc<dyn RuntimeApi>, cancel: &CancellationToken, ms: u64) {
    let deadline = MonoTime::from_micros(runtime_api.now().as_micros() + ms * 1000);
    let mut timer = runtime_api.sleep_until(deadline);
    let cancel_fut = cancel.cancelled().fuse();
    let timer_fut = timer.wait().fuse();
    pin_mut!(cancel_fut, timer_fut);
    select_biased! {
        _ = cancel_fut => {},
        _ = timer_fut => {},
    }
}

fn map_http_flv_pull_error(err: HttpFlvPullError) -> SdkError {
    match err {
        HttpFlvPullError::InvalidUrl(_) | HttpFlvPullError::UnsupportedScheme { .. } => {
            SdkError::InvalidArgument(err.to_string())
        }
        HttpFlvPullError::Cancelled => SdkError::Unavailable(err.to_string()),
        _ => SdkError::Internal(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_runtime_tokio::TokioRuntime;

    #[test]
    fn options_default() {
        let opts = HttpFlvSubscriberOptions::default();
        assert_eq!(opts.buffer_size, 64);
        assert!(opts.reconnect.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn open_rejects_invalid_url() {
        let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
        let err =
            open_http_flv_subscriber(runtime, "ftp://example.com/live.flv", Default::default())
                .await
                .expect_err("ftp must be unsupported");
        assert!(matches!(err, HttpFlvPullError::UnsupportedScheme { .. }));
    }
}
