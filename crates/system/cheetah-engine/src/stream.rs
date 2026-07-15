use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use arc_swap::{ArcSwap, ArcSwapOption};
use async_trait::async_trait;
use cheetah_codec::{AVFrame, FrameFlags, TrackInfo};
use cheetah_media_api::event::{
    MediaEvent, MediaEventBusApi, StreamOnlineChanged, StreamPublished, StreamUnpublished,
};
use cheetah_media_api::ids::{MediaKey, MediaSchema, SessionId, StreamKeyBridge};
use cheetah_media_api::model::{CloseReason, OnlineState};
use cheetah_sdk::{
    BackpressurePolicy, BootstrapMode, BootstrapPolicy, DispatchResult, EventBus, MediaFilter,
    PublishLease, PublisherApi, PublisherOptions, PublisherSink, RuntimeApi, SdkError, StreamEvent,
    StreamEventKind, StreamId, StreamKey, StreamManagerApi, StreamSnapshot, SubscriberApi,
    SubscriberId, SubscriberOptions, SubscriberSource, SystemEvent,
};
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc;

/// Frame dispatch strategy for the whole stream manager.
///
/// `PerStream` keeps a dedicated dispatcher per stream; `SharedPool` shares a
/// fixed worker pool across streams to reduce task count.
///
/// 流管理器使用的帧分发策略。
///
/// `PerStream` 为每个流保留独立分发器；`SharedPool` 则让多个流共享固定工作池以减少任务数。
#[derive(Debug, Clone, Copy, Default)]
pub enum DispatcherMode {
    #[default]
    PerStream,
    SharedPool {
        workers: usize,
    },
}

/// Position and version of a keyframe (IDR) stored in the ring buffer.
///
/// Used to find valid random-access points for GOP bootstrap.
///
/// 环形缓冲区中关键帧（IDR）的位置与版本。
///
/// 用于为 GOP 引导找到有效的随机访问点。
#[derive(Debug, Clone)]
struct IdrNode {
    ring_pos: usize,
    slot_version: u64,
}

/// One slot in the lock-free ring buffer.
///
/// A slot stores the current `AVFrame` and a monotonic version counter.
///
/// 无锁环形缓冲区中的一个槽位。
///
/// 每个槽位存储当前 `AVFrame` 和单调递增的版本计数。
struct RingSlot {
    frame: ArcSwapOption<AVFrame>,
    version: AtomicU64,
}

impl RingSlot {
    /// Create an empty slot with version zero.
    ///
    /// 创建版本为 0 的空槽位。
    fn new() -> Self {
        Self {
            frame: ArcSwapOption::const_empty(),
            version: AtomicU64::new(0),
        }
    }
}

/// Lock-free ring buffer that holds recent frames for a single stream.
///
/// Frames are overwritten after the capacity wraps. The buffer maintains an
/// `idr_list` of keyframe positions so subscribers can bootstrap from a GOP.
///
/// 单流的无锁环形缓冲区，保存最近帧。
///
/// 容量环绕后旧帧会被覆盖。缓冲区维护关键帧位置列表，使订阅者能从 GOP 起始播放。
struct RingBuffer {
    slots: Vec<RingSlot>,
    mask: usize,
    write_pos: AtomicUsize,
    idr_list: ArcSwap<Vec<IdrNode>>,
    idr_write_lock: Mutex<()>,
}

impl RingBuffer {
    /// Create a ring buffer with capacity rounded up to the next power of two.
    ///
    /// 创建容量向上取整为 2 的幂的环形缓冲区。
    fn new(capacity: usize) -> Self {
        let cap = capacity.max(2).next_power_of_two();
        let slots = (0..cap).map(|_| RingSlot::new()).collect();
        Self {
            slots,
            mask: cap - 1,
            write_pos: AtomicUsize::new(0),
            idr_list: ArcSwap::from_pointee(Vec::new()),
            idr_write_lock: Mutex::new(()),
        }
    }

    /// Append a frame, update its slot version, and record keyframes.
    ///
    /// Returns the ring position and the new slot version.
    ///
    /// 追加帧，更新槽位版本，并记录关键帧。
    ///
    /// 返回环形位置与新的槽位版本。
    fn push(&self, frame: Arc<AVFrame>) -> (usize, u64) {
        let pos = self.write_pos.fetch_add(1, Ordering::AcqRel);
        let slot_idx = pos & self.mask;
        let slot = &self.slots[slot_idx];

        let version = slot.version.fetch_add(1, Ordering::AcqRel) + 1;
        slot.frame.store(Some(frame.clone()));

        if frame.flags.contains(FrameFlags::KEY) {
            self.record_idr(pos, version);
        }

        (pos, version)
    }

    /// Earliest position still valid for reading given the current write end.
    ///
    /// 给定当前写入端，仍可读到的最早位置。
    fn earliest_readable_pos(&self, end: usize) -> usize {
        end.saturating_sub(self.slots.len())
    }

    /// Remove IDR entries that have been overwritten or no longer match the slot version.
    ///
    /// 删除已被覆盖或槽位版本不再匹配的 IDR 条目。
    fn prune_idr_nodes(&self, list: &mut Vec<IdrNode>, end: usize) {
        let min_pos = self.earliest_readable_pos(end);
        list.retain(|node| {
            if node.ring_pos < min_pos || node.ring_pos >= end {
                return false;
            }
            let slot_idx = node.ring_pos & self.mask;
            let slot = &self.slots[slot_idx];
            slot.version.load(Ordering::Acquire) == node.slot_version
        });
    }

    /// Append a new IDR node and prune stale entries under the write lock.
    ///
    /// 在写锁下追加新的 IDR 节点并裁剪过期条目。
    fn record_idr(&self, ring_pos: usize, slot_version: u64) {
        let _guard = self.idr_write_lock.lock();
        let mut list = (*self.idr_list.load_full()).clone();
        list.push(IdrNode {
            ring_pos,
            slot_version,
        });

        let end = self.write_pos.load(Ordering::Acquire);
        self.prune_idr_nodes(&mut list, end);

        let max_entries = self.slots.len() * 2;
        if list.len() > max_entries {
            let keep_from = list.len() - max_entries;
            list.drain(0..keep_from);
        }

        self.idr_list.store(Arc::new(list));
    }

    /// Read a frame at a given ring position if the slot version is stable.
    ///
    /// 若槽位版本稳定，则读取指定环形位置的帧。
    fn read(&self, ring_pos: usize) -> Option<(Arc<AVFrame>, u64)> {
        let slot_idx = ring_pos & self.mask;
        let slot = &self.slots[slot_idx];

        let version_before = slot.version.load(Ordering::Acquire);
        let frame = slot.frame.load_full()?;
        let version_after = slot.version.load(Ordering::Acquire);

        if version_before != version_after {
            return None;
        }

        Some((frame, version_after))
    }

    /// Collect bootstrap frames for a new subscriber according to the requested policy.
    ///
    /// The start position is clamped by age, discontinuity, and random-access policy.
    ///
    /// 根据请求策略为新订阅者收集引导帧。
    ///
    /// 起始位置会受最大年龄、 discontinuity 和随机访问策略限制。
    fn bootstrap_frames(&self, policy: BootstrapPolicy) -> Vec<Arc<AVFrame>> {
        if matches!(policy.mode, BootstrapMode::None) || policy.max_bootstrap_frames == 0 {
            return Vec::new();
        }

        let end = self.write_pos.load(Ordering::Acquire);
        let mut start = end.saturating_sub(policy.max_bootstrap_frames);
        start = self.clamp_start_by_max_age(start, end, policy.max_bootstrap_age_ms);
        start = self.clamp_start_by_discontinuity(start, end);
        start = self.clamp_start_by_random_access(start, end, policy);

        let mut out = Vec::new();
        for ring_pos in start..end {
            if let Some((frame, _)) = self.read(ring_pos) {
                out.push(frame);
            }
        }
        tracing::info!(
            bootstrap_start = start,
            bootstrap_end = end,
            bootstrap_count = out.len(),
            "ring bootstrap frames"
        );
        out
    }

    /// Clamp the bootstrap start to an IDR keyframe if the policy requires it.
    ///
    /// `LiveTail` picks the latest IDR in range; `FullGop` picks the earliest.
    ///
    /// 若策略要求，则将引导起始位置限制到 IDR 关键帧。
    ///
    /// `LiveTail` 选择范围内最新的 IDR；`FullGop` 选择最早的。
    fn clamp_start_by_random_access(
        &self,
        start: usize,
        end: usize,
        policy: BootstrapPolicy,
    ) -> usize {
        if !matches!(
            policy.mode,
            BootstrapMode::LiveTail | BootstrapMode::FullGop
        ) {
            return start;
        }

        let _guard = self.idr_write_lock.lock();
        let mut list = (*self.idr_list.load_full()).clone();
        self.prune_idr_nodes(&mut list, end);
        let chosen = match policy.mode {
            BootstrapMode::LiveTail => list
                .iter()
                .rev()
                .find(|node| node.ring_pos >= start && node.ring_pos < end)
                .map(|node| node.ring_pos),
            BootstrapMode::FullGop => list
                .iter()
                .find(|node| node.ring_pos >= start && node.ring_pos < end)
                .map(|node| node.ring_pos),
            BootstrapMode::None => None,
        };
        self.idr_list.store(Arc::new(list));

        match chosen {
            Some(ring_pos) => ring_pos,
            None if policy.wait_for_next_random_access_point => end,
            None => start,
        }
    }

    /// Clamp the bootstrap start so that frames are not older than `max_bootstrap_age_ms`.
    ///
    /// 限制引导起始位置，使帧不早于 `max_bootstrap_age_ms`。
    fn clamp_start_by_max_age(
        &self,
        start: usize,
        end: usize,
        max_bootstrap_age_ms: Option<u64>,
    ) -> usize {
        let Some(max_age_ms) = max_bootstrap_age_ms else {
            return start;
        };
        if max_age_ms == 0 || start >= end {
            return end;
        }

        let latest_ts_ms = (start..end)
            .rev()
            .find_map(|ring_pos| self.read(ring_pos))
            .and_then(|(frame, _)| frame_time_ms(frame.as_ref()));
        let Some(latest_ts_ms) = latest_ts_ms else {
            return start;
        };

        let min_ts_ms = latest_ts_ms.saturating_sub(max_age_ms as i128);
        for ring_pos in (start..end).rev() {
            let Some((frame, _)) = self.read(ring_pos) else {
                continue;
            };
            let Some(ts_ms) = frame_time_ms(frame.as_ref()) else {
                continue;
            };
            if ts_ms < min_ts_ms {
                return ring_pos.saturating_add(1);
            }
        }
        start
    }

    /// Clamp the bootstrap start to the most recent discontinuity marker.
    ///
    /// 将引导起始位置限制到最近的不连续标记。
    fn clamp_start_by_discontinuity(&self, start: usize, end: usize) -> usize {
        for ring_pos in (start..end).rev() {
            let Some((frame, _)) = self.read(ring_pos) else {
                continue;
            };
            if frame.flags.contains(FrameFlags::DISCONTINUITY) {
                return ring_pos;
            }
        }
        start
    }
}

/// Convert a frame timestamp to milliseconds for age-based bootstrap clamping.
///
/// 将帧时间戳转换为毫秒，用于基于年龄的引导限制。
fn frame_time_ms(frame: &AVFrame) -> Option<i128> {
    let ts = if frame.dts >= 0 {
        frame.dts
    } else if frame.pts >= 0 {
        frame.pts
    } else {
        return None;
    };
    let numer = i128::from(ts).checked_mul(i128::from(frame.timebase.num))?;
    let denom = i128::from(frame.timebase.den);
    if denom <= 0 {
        return None;
    }
    numer.checked_mul(1_000)?.checked_div(denom)
}

/// A single subscriber connected to a dispatcher.
///
/// 连接到分发器的单个订阅者。
#[derive(Clone)]
struct DispatchSubscriber {
    id: SubscriberId,
    tx: mpsc::Sender<Arc<AVFrame>>,
    policy: BackpressurePolicy,
    media_filter: MediaFilter,
    wait_for_next_keyframe: Arc<AtomicBool>,
}

/// Shared inner state for `Dispatcher`.
///
/// Holds the subscriber list and assigns monotonic subscriber IDs.
///
/// `Dispatcher` 的共享内部状态。
///
/// 保存订阅者列表并分配单调递增订阅者 ID。
struct DispatcherInner {
    subscribers: ArcSwap<Vec<DispatchSubscriber>>,
    next_subscriber_id: AtomicU64,
}

impl DispatcherInner {
    /// Create a dispatcher with an empty subscriber list.
    ///
    /// 创建订阅者列表为空的分发器。
    fn new() -> Self {
        Self {
            subscribers: ArcSwap::from_pointee(Vec::new()),
            next_subscriber_id: AtomicU64::new(0),
        }
    }

    /// Add a new subscriber with its own async channel and return the receiver.
    ///
    /// 添加新的订阅者，为其分配异步通道并返回接收端。
    fn add_subscriber(
        &self,
        options: &SubscriberOptions,
    ) -> (DispatchSubscriber, mpsc::Receiver<Arc<AVFrame>>) {
        let id = SubscriberId(self.next_subscriber_id.fetch_add(1, Ordering::Relaxed) + 1);
        let (tx, rx) = mpsc::channel(options.queue_capacity.max(1));
        let sub = DispatchSubscriber {
            id,
            tx,
            policy: options.backpressure,
            media_filter: options.media_filter,
            wait_for_next_keyframe: Arc::new(AtomicBool::new(false)),
        };

        let mut next = (*self.subscribers.load_full()).clone();
        next.push(sub.clone());
        self.subscribers.store(Arc::new(next));

        (sub, rx)
    }

    /// Remove subscribers matching the given IDs.
    ///
    /// 移除匹配的订阅者。
    fn remove_subscribers(&self, ids: &HashSet<SubscriberId>) {
        if ids.is_empty() {
            return;
        }
        let mut next = (*self.subscribers.load_full()).clone();
        next.retain(|sub| !ids.contains(&sub.id));
        self.subscribers.store(Arc::new(next));
    }

    /// Remove all subscribers at once.
    ///
    /// 一次性移除所有订阅者。
    fn clear_subscribers(&self) {
        self.subscribers.store(Arc::new(Vec::new()));
    }

    /// Dispatch one frame to all subscribers while applying media filters and backpressure.
    ///
    /// 将单帧分发给所有订阅者，同时应用媒体过滤与背压策略。
    fn dispatch_frame(&self, frame: Arc<AVFrame>) -> DispatchResult {
        let subs = self.subscribers.load();
        if subs.is_empty() {
            return DispatchResult::RejectedClosed;
        }

        let is_key = frame.flags.contains(FrameFlags::KEY);
        let mut accepted = false;
        let mut dropped = false;
        let mut remove_ids = HashSet::new();

        for sub in subs.iter() {
            // Media filter: skip frames the subscriber doesn't want
            if !sub.media_filter.enable_video && frame.media_kind == cheetah_codec::MediaKind::Video
            {
                continue;
            }
            if !sub.media_filter.enable_audio && frame.media_kind == cheetah_codec::MediaKind::Audio
            {
                continue;
            }

            if sub.policy == BackpressurePolicy::DropUntilNextKeyframe
                && sub.wait_for_next_keyframe.load(Ordering::Acquire)
            {
                if is_key {
                    sub.wait_for_next_keyframe.store(false, Ordering::Release);
                } else {
                    dropped = true;
                    continue;
                }
            }

            match sub.tx.try_send(frame.clone()) {
                Ok(_) => {
                    accepted = true;
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => match sub.policy {
                    BackpressurePolicy::DropDroppableFirst => {
                        if !is_key || frame.flags.contains(FrameFlags::DROPPABLE) {
                            dropped = true;
                        } else {
                            remove_ids.insert(sub.id);
                        }
                    }
                    BackpressurePolicy::DropUntilNextKeyframe => {
                        sub.wait_for_next_keyframe.store(true, Ordering::Release);
                        dropped = true;
                    }
                    BackpressurePolicy::DisconnectOnOverflow => {
                        dropped = true;
                        remove_ids.insert(sub.id);
                    }
                },
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    remove_ids.insert(sub.id);
                }
            }
        }

        self.remove_subscribers(&remove_ids);

        if accepted {
            DispatchResult::Accepted
        } else if dropped {
            DispatchResult::DroppedByPolicy
        } else {
            DispatchResult::RejectedClosed
        }
    }

    /// Number of currently attached subscribers.
    ///
    /// 当前连接的订阅者数量。
    fn subscriber_count(&self) -> usize {
        self.subscribers.load().len()
    }
}

/// Per-stream dispatcher wrapping `DispatcherInner`.
///
/// 每个流对应的分发器，封装 `DispatcherInner`。
#[derive(Clone)]
struct Dispatcher {
    inner: Arc<DispatcherInner>,
}

impl Dispatcher {
    /// Create a dispatcher wrapping a fresh `DispatcherInner`.
    ///
    /// 创建包装新 `DispatcherInner` 的分发器。
    fn new() -> Self {
        Self {
            inner: Arc::new(DispatcherInner::new()),
        }
    }

    /// Forward subscriber creation to the inner dispatcher.
    ///
    /// 将订阅者创建转发到内部分发器。
    fn add_subscriber(
        &self,
        options: &SubscriberOptions,
    ) -> (DispatchSubscriber, mpsc::Receiver<Arc<AVFrame>>) {
        self.inner.add_subscriber(options)
    }

    /// Remove a single subscriber by ID.
    ///
    /// 根据 ID 移除单个订阅者。
    fn remove_subscriber(&self, id: SubscriberId) {
        let mut ids = HashSet::new();
        ids.insert(id);
        self.inner.remove_subscribers(&ids);
    }

    /// Remove all subscribers at once.
    ///
    /// 一次性移除所有订阅者。
    fn clear_subscribers(&self) {
        self.inner.clear_subscribers();
    }

    /// Number of currently attached subscribers.
    ///
    /// 当前连接的订阅者数量。
    fn subscriber_count(&self) -> usize {
        self.inner.subscriber_count()
    }

    /// Dispatch a frame either locally or via the shared dispatch pool.
    ///
    /// 在本地或通过共享分发池分发帧。
    fn dispatch(&self, mode: &StreamDispatchMode, frame: Arc<AVFrame>) -> DispatchResult {
        match mode {
            StreamDispatchMode::PerStream => self.inner.dispatch_frame(frame),
            StreamDispatchMode::Shared { pool, lane } => {
                pool.enqueue(*lane, self.inner.clone(), frame)
            }
        }
    }
}

/// Job sent to a worker in the shared dispatch pool.
///
/// 发送给共享分发池工作线程的任务。
struct DispatchJob {
    dispatcher: Arc<DispatcherInner>,
    frame: Arc<AVFrame>,
}

/// Shared pool of workers that dispatch frames for multiple streams.
///
/// 多个流共享的分发工作线程池。
struct SharedDispatchPool {
    workers: Vec<mpsc::Sender<DispatchJob>>,
}

impl SharedDispatchPool {
    /// Spawn `worker_count` background tasks that pull and dispatch frames.
    ///
    /// 生成 `worker_count` 个后台任务，拉取并分发帧。
    fn new(worker_count: usize, runtime_api: Arc<dyn RuntimeApi>) -> Self {
        let mut workers = Vec::new();
        let count = worker_count.max(1);

        for _ in 0..count {
            let (tx, mut rx) = mpsc::channel::<DispatchJob>(2048);
            let _ = runtime_api.spawn(Box::pin(async move {
                while let Some(job) = rx.recv().await {
                    let _ = job.dispatcher.dispatch_frame(job.frame);
                }
            }));
            workers.push(tx);
        }

        Self { workers }
    }

    /// Map a stream id to a worker lane to preserve stream ordering.
    ///
    /// 将流 ID 映射到工作线程通道，以保持单流顺序。
    fn lane_of_stream(&self, stream_id: StreamId) -> usize {
        (stream_id.0 as usize) % self.workers.len()
    }

    /// Enqueue a frame dispatch job on a worker lane.
    ///
    /// 将帧分发任务入队到指定工作线程通道。
    fn enqueue(
        &self,
        lane: usize,
        dispatcher: Arc<DispatcherInner>,
        frame: Arc<AVFrame>,
    ) -> DispatchResult {
        let idx = lane % self.workers.len();
        match self.workers[idx].try_send(DispatchJob { dispatcher, frame }) {
            Ok(_) => DispatchResult::Accepted,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => DispatchResult::DroppedByPolicy,
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                DispatchResult::RejectedClosed
            }
        }
    }
}

/// Dispatch mode chosen for a single stream.
///
/// 为单个流选择的分发模式。
#[derive(Clone)]
enum StreamDispatchMode {
    PerStream,
    Shared {
        pool: Arc<SharedDispatchPool>,
        lane: usize,
    },
}

/// Per-stream state: ring buffer, dispatcher, tracks, and active lease.
///
/// 单流状态：环形缓冲区、分发器、轨道信息和活跃租约。
struct StreamEntry {
    stream_id: StreamId,
    ring: RingBuffer,
    dispatcher: Dispatcher,
    tracks: RwLock<Vec<TrackInfo>>,
    dispatch_mode: StreamDispatchMode,
    active_lease: AtomicU64,
    keyframe_requests: AtomicU64,
    /// Timestamp (micros since epoch) when subscriber count last dropped to zero.
    /// 0 means there are currently subscribers or no subscriber has ever joined.
    last_no_subscriber_micros: AtomicU64,
    /// Protocol identifier reported with stream events.
    protocol: Mutex<String>,
    /// Optional remote endpoint reported with stream events.
    remote_endpoint: Mutex<Option<String>>,
    /// Whether `StreamPublished` has already been emitted for this entry.
    published: AtomicBool,
    /// Current online state, used to avoid duplicate `StreamOnlineChanged` events.
    online: AtomicBool,
}

impl StreamEntry {
    /// Create a new `StreamEntry` with an empty ring and dispatcher.
    ///
    /// 创建新的 `StreamEntry`，其环形缓冲区与分发器为空。
    fn new(stream_id: StreamId, ring_capacity: usize, dispatch_mode: StreamDispatchMode) -> Self {
        Self {
            stream_id,
            ring: RingBuffer::new(ring_capacity),
            dispatcher: Dispatcher::new(),
            tracks: RwLock::new(Vec::new()),
            dispatch_mode,
            active_lease: AtomicU64::new(0),
            keyframe_requests: AtomicU64::new(0),
            last_no_subscriber_micros: AtomicU64::new(0),
            protocol: Mutex::new(String::new()),
            remote_endpoint: Mutex::new(None),
            published: AtomicBool::new(false),
            online: AtomicBool::new(false),
        }
    }
}

/// Internal state of `StreamManager`, indexed by `StreamKey`.
///
/// `StreamManager` 的内部状态，按 `StreamKey` 索引。
struct StreamManagerInner {
    mode: DispatcherMode,
    ring_capacity: usize,
    shared_pool: Option<Arc<SharedDispatchPool>>,
    next_stream_id: AtomicU64,
    next_lease_id: AtomicU64,
    streams: DashMap<StreamKey, Arc<StreamEntry>>,
    event_bus: RwLock<Option<Arc<dyn EventBus>>>,
    media_event_bus: RwLock<Option<Arc<dyn MediaEventBusApi>>>,
}

impl StreamManagerInner {
    /// Publish a `StreamEvent` to the event bus if one is configured.
    ///
    /// 若已配置事件总线，则发布 `StreamEvent`。
    fn publish_stream_event(
        &self,
        stream_key: &StreamKey,
        kind: StreamEventKind,
        stream_id: Option<StreamId>,
        subscriber_id: Option<SubscriberId>,
        dispatch_result: Option<DispatchResult>,
        message: Option<String>,
    ) {
        if let Some(event_bus) = self.event_bus.read().as_ref() {
            event_bus.publish(SystemEvent::Stream(StreamEvent {
                stream_key: stream_key.to_string(),
                kind,
                stream_id: stream_id.map(|id| id.0),
                subscriber_id: subscriber_id.map(|id| id.0),
                dispatch_result,
                message,
            }));
        }
    }

    /// Publish a typed `MediaEvent` if a media event bus is configured.
    ///
    /// 若已配置媒体事件总线，则发布类型化 `MediaEvent`。
    fn publish_media_event(&self, event: MediaEvent) {
        if let Some(bus) = self.media_event_bus.read().as_ref() {
            let _ = bus.publish(event);
        }
    }

    /// Convert a `StreamKey` to a `MediaKey` for event headers.
    fn media_key_for(stream_key: &StreamKey) -> MediaKey {
        StreamKeyBridge::from_namespace_path(&stream_key.namespace, &stream_key.path)
            .unwrap_or_else(|_| {
                MediaKey::new(
                    "__fallback__",
                    &stream_key.namespace,
                    &stream_key.path,
                    None,
                )
                .unwrap_or(MediaKey::with_default_vhost("unknown", "unknown", None).unwrap())
            })
    }

    /// Pick a `MediaSchema` from the entry metadata for `StreamOnlineChanged`.
    fn schema_for(entry: &StreamEntry, media_key: &MediaKey) -> Option<MediaSchema> {
        if let Some(schema) = media_key.schema {
            return Some(schema);
        }
        let protocol = entry.protocol.lock();
        MediaSchema::parse(&protocol).ok()
    }

    /// Emit `StreamPublished` and `StreamOnlineChanged(Online)` once per entry.
    fn publish_stream_published(&self, stream_key: &StreamKey, entry: &StreamEntry) {
        if entry.published.swap(true, Ordering::AcqRel) {
            return;
        }
        let media_key = Self::media_key_for(stream_key);
        let protocol = entry.protocol.lock().clone();
        let remote_endpoint = entry.remote_endpoint.lock().clone();
        let session_id = SessionId(entry.stream_id.0.to_string());
        let mut header =
            crate::media_provider::util::event_header("stream-manager", Some(&media_key), None);
        header.correlation_id = Some(session_id.0.clone());
        self.publish_media_event(MediaEvent::StreamPublished(StreamPublished {
            header,
            protocol,
            remote_endpoint,
            session_id,
        }));
        if entry.online.swap(true, Ordering::AcqRel) {
            return;
        }
        let schema = Self::schema_for(entry, &media_key);
        self.publish_media_event(MediaEvent::StreamOnlineChanged(StreamOnlineChanged {
            header: crate::media_provider::util::event_header(
                "stream-manager",
                Some(&media_key),
                None,
            ),
            online: OnlineState::Online,
            schema,
        }));
    }

    /// Emit `StreamUnpublished` and `StreamOnlineChanged(Offline)` if the stream was published.
    fn publish_stream_unpublished(
        &self,
        stream_key: &StreamKey,
        entry: &StreamEntry,
        reason: CloseReason,
    ) {
        let media_key = Self::media_key_for(stream_key);
        let session_id = SessionId(entry.stream_id.0.to_string());
        let was_published = entry.published.swap(false, Ordering::AcqRel);
        if was_published {
            let mut header =
                crate::media_provider::util::event_header("stream-manager", Some(&media_key), None);
            header.correlation_id = Some(session_id.0.clone());
            self.publish_media_event(MediaEvent::StreamUnpublished(StreamUnpublished {
                header,
                session_id,
                reason,
            }));
        }
        if entry.online.swap(false, Ordering::AcqRel) {
            let schema = Self::schema_for(entry, &media_key);
            self.publish_media_event(MediaEvent::StreamOnlineChanged(StreamOnlineChanged {
                header: crate::media_provider::util::event_header(
                    "stream-manager",
                    Some(&media_key),
                    None,
                ),
                online: OnlineState::Offline,
                schema,
            }));
        }
    }

    /// Choose the dispatch mode for a new stream based on the manager's mode.
    ///
    /// 根据管理模式为新流选择分发模式。
    fn resolve_dispatch_mode(&self, stream_id: StreamId) -> StreamDispatchMode {
        match &self.shared_pool {
            Some(pool) => StreamDispatchMode::Shared {
                pool: pool.clone(),
                lane: pool.lane_of_stream(stream_id),
            },
            None => StreamDispatchMode::PerStream,
        }
    }

    /// Build a public `StreamSnapshot` from the entry state.
    ///
    /// 从条目状态构建公共 `StreamSnapshot`。
    fn snapshot_for(&self, stream_key: &StreamKey, entry: &StreamEntry) -> StreamSnapshot {
        StreamSnapshot {
            stream_id: entry.stream_id,
            key: stream_key.clone(),
            publisher_active: entry.active_lease.load(Ordering::Acquire) != 0,
            subscriber_count: entry.dispatcher.subscriber_count(),
            tracks: entry.tracks.read().clone(),
        }
    }

    /// Return an existing stream entry or create a new one atomically.
    ///
    /// 返回已有流条目，或以原子方式创建新条目。
    fn get_or_create_stream(&self, stream_key: StreamKey) -> Arc<StreamEntry> {
        if let Some(entry) = self.streams.get(&stream_key) {
            return Arc::clone(entry.value());
        }

        let stream_id = StreamId(self.next_stream_id.fetch_add(1, Ordering::Relaxed) + 1);
        let entry = Arc::new(StreamEntry::new(
            stream_id,
            self.ring_capacity,
            self.resolve_dispatch_mode(stream_id),
        ));

        match self.streams.entry(stream_key) {
            dashmap::mapref::entry::Entry::Occupied(occupied) => Arc::clone(occupied.get()),
            dashmap::mapref::entry::Entry::Vacant(vacant) => {
                vacant.insert(Arc::clone(&entry));
                entry
            }
        }
    }

    /// Remove a stream from the map if it has no publisher and no subscribers.
    ///
    /// 若流没有发布者和订阅者，则将其从映射中移除。
    fn cleanup_if_idle(&self, stream_key: &StreamKey) {
        let should_remove = self
            .streams
            .get(stream_key)
            .map(|entry| {
                let no_publisher = entry.value().active_lease.load(Ordering::Acquire) == 0;
                let no_subscribers = entry.value().dispatcher.subscriber_count() == 0;
                if no_subscribers && !no_publisher {
                    // Record when subscribers dropped to zero (for idle timeout)
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_micros() as u64;
                    entry
                        .value()
                        .last_no_subscriber_micros
                        .store(now, Ordering::Relaxed);
                }
                no_publisher && no_subscribers
            })
            .unwrap_or(false);

        if should_remove {
            self.streams.remove(stream_key);
        }
    }

    /// Release a publisher lease and close the stream if the lease ID matches.
    ///
    /// 若租约 ID 匹配，则释放发布者租约并关闭流。
    fn release_lease(&self, stream_key: &StreamKey, lease_id: u64) -> Result<(), SdkError> {
        let entry = self
            .streams
            .get(stream_key)
            .map(|v| Arc::clone(v.value()))
            .ok_or_else(|| SdkError::NotFound(format!("stream {stream_key}")))?;

        let current = entry.active_lease.load(Ordering::Acquire);
        if current != lease_id {
            return Err(SdkError::Conflict(format!(
                "publisher lease mismatch for stream {}",
                stream_key
            )));
        }

        entry.active_lease.store(0, Ordering::Release);
        entry.dispatcher.clear_subscribers();
        self.publish_stream_unpublished(stream_key, &entry, CloseReason::Normal);
        self.streams.remove(stream_key);
        self.publish_stream_event(
            stream_key,
            StreamEventKind::StreamClosed,
            Some(entry.stream_id),
            None,
            None,
            None,
        );
        Ok(())
    }
}

/// Runtime-neutral implementation of the stream manager API.
///
/// Owns `StreamEntry` instances keyed by `StreamKey`, and implements `PublisherApi`,
/// `SubscriberApi`, and `StreamManagerApi`.
///
/// 流管理器 API 的运行时无关实现。
///
/// 拥有按 `StreamKey` 索引的 `StreamEntry` 实例，并实现 `PublisherApi`、`SubscriberApi` 与 `StreamManagerApi`。
pub struct StreamManager {
    inner: Arc<StreamManagerInner>,
}

impl StreamManager {
    /// Create a stream manager with the given dispatcher mode and ring capacity.
    ///
    /// 用指定分发模式和环形容量创建流管理器。
    pub fn new(
        mode: DispatcherMode,
        ring_capacity: usize,
        runtime_api: Arc<dyn RuntimeApi>,
    ) -> Self {
        let shared_pool = match mode {
            DispatcherMode::PerStream => None,
            DispatcherMode::SharedPool { workers } => Some(Arc::new(SharedDispatchPool::new(
                workers.max(1),
                runtime_api,
            ))),
        };
        Self {
            inner: Arc::new(StreamManagerInner {
                mode,
                ring_capacity: ring_capacity.max(128),
                shared_pool,
                next_stream_id: AtomicU64::new(0),
                next_lease_id: AtomicU64::new(0),
                streams: DashMap::new(),
                event_bus: RwLock::new(None),
                media_event_bus: RwLock::new(None),
            }),
        }
    }

    /// Attach the event bus used for stream lifecycle events.
    ///
    /// 附加用于流生命周期事件的事件总线。
    pub fn set_event_bus(&self, event_bus: Arc<dyn EventBus>) {
        *self.inner.event_bus.write() = Some(event_bus);
    }

    /// Attach the typed media event bus used for `MediaEvent` publish/subscribe.
    ///
    /// 附加用于 `MediaEvent` 发布/订阅的类型化媒体事件总线。
    pub fn set_media_event_bus(&self, media_event_bus: Arc<dyn MediaEventBusApi>) {
        *self.inner.media_event_bus.write() = Some(media_event_bus);
    }
}

/// Handle for an active publisher, enforcing single-lease semantics.
///
/// 活跃发布者句柄，强制单租约语义。
struct PublisherHandle {
    inner: Arc<StreamManagerInner>,
    stream_key: StreamKey,
    entry: Arc<StreamEntry>,
    lease_id: u64,
    closed: AtomicBool,
}

impl PublisherHandle {
    /// Check that the publisher lease is still valid and the handle is not closed.
    ///
    /// 检查发布者租约是否仍然有效且句柄未关闭。
    fn ensure_active(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
            && self.entry.active_lease.load(Ordering::Acquire) == self.lease_id
    }
}

/// `PublisherSink` implementation that writes to the stream ring and dispatcher.
///
/// `PublisherSink` 实现，写入流环形缓冲区与分发器。
impl PublisherSink for PublisherHandle {
    fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
        if !self.ensure_active() {
            return Err(SdkError::Unavailable(format!(
                "publisher already closed for {}",
                self.stream_key
            )));
        }
        *self.entry.tracks.write() = tracks.clone();
        if !tracks.is_empty() {
            self.inner
                .publish_stream_published(&self.stream_key, &self.entry);
        }
        Ok(())
    }

    fn push_frame(&self, frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError> {
        if !self.ensure_active() {
            return Ok(DispatchResult::RejectedClosed);
        }

        self.entry.ring.push(frame.clone());
        let result = self
            .entry
            .dispatcher
            .dispatch(&self.entry.dispatch_mode, frame);
        if result == DispatchResult::DroppedByPolicy {
            self.inner.publish_stream_event(
                &self.stream_key,
                StreamEventKind::FrameDropped,
                Some(self.entry.stream_id),
                None,
                Some(result),
                Some("backpressure".to_string()),
            );
        }
        Ok(result)
    }

    fn close(&self) -> Result<(), SdkError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let _ = self.inner.release_lease(&self.stream_key, self.lease_id);
        Ok(())
    }

    fn take_keyframe_requests(&self) -> u64 {
        self.entry.keyframe_requests.swap(0, Ordering::Relaxed)
    }
}

/// Drop the publisher handle, releasing the lease.
///
/// 释放发布者句柄时释放租约。
impl Drop for PublisherHandle {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// Handle for a subscriber, holding its async channel and dispatcher reference.
///
/// 订阅者句柄，持有其异步通道与分发器引用。
struct SubscriberHandle {
    inner: Arc<StreamManagerInner>,
    stream_key: StreamKey,
    stream_id: StreamId,
    id: SubscriberId,
    rx: mpsc::Receiver<Arc<AVFrame>>,
    dispatcher: Dispatcher,
    closed: bool,
}

impl SubscriberHandle {
    /// Remove the subscriber and trigger idle cleanup.
    ///
    /// 移除订阅者并触发空闲清理。
    fn close_inner(&mut self) {
        if self.closed {
            return;
        }
        self.dispatcher.remove_subscriber(self.id);
        self.inner.publish_stream_event(
            &self.stream_key,
            StreamEventKind::SubscriberClosed,
            Some(self.stream_id),
            Some(self.id),
            None,
            None,
        );
        self.inner.cleanup_if_idle(&self.stream_key);
        self.closed = true;
    }
}

/// `SubscriberSource` implementation that receives frames from the dispatcher channel.
///
/// `SubscriberSource` 实现，从分发器通道接收帧。
#[async_trait]
impl SubscriberSource for SubscriberHandle {
    async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, SdkError> {
        if self.closed {
            return Ok(None);
        }
        Ok(self.rx.recv().await)
    }

    async fn close(&mut self) -> Result<(), SdkError> {
        self.close_inner();
        Ok(())
    }

    fn id(&self) -> SubscriberId {
        self.id
    }
}

/// Drop the subscriber handle, closing the subscription.
///
/// 释放订阅者句柄时关闭订阅。
impl Drop for SubscriberHandle {
    fn drop(&mut self) {
        self.close_inner();
    }
}

/// `PublisherApi` implementation: single-publisher lease acquisition per stream.
///
/// `PublisherApi` 实现：每流仅允许一个发布者租约。
#[async_trait]
impl PublisherApi for StreamManager {
    async fn acquire_publisher(
        &self,
        stream_key: StreamKey,
        options: PublisherOptions,
    ) -> Result<(PublishLease, Box<dyn PublisherSink>), SdkError> {
        let entry = self.inner.get_or_create_stream(stream_key.clone());
        let lease_id = self.inner.next_lease_id.fetch_add(1, Ordering::Relaxed) + 1;
        let previous =
            entry
                .active_lease
                .compare_exchange(0, lease_id, Ordering::AcqRel, Ordering::Acquire);
        if previous.is_err() {
            return Err(SdkError::Conflict(format!(
                "stream {} already has an active publisher",
                stream_key
            )));
        }
        *entry.protocol.lock() = options.protocol;
        *entry.remote_endpoint.lock() = options.remote_endpoint;
        self.inner.publish_stream_event(
            &stream_key,
            StreamEventKind::PublisherOpened,
            Some(entry.stream_id),
            None,
            None,
            None,
        );
        if !entry.online.swap(true, Ordering::AcqRel) {
            let media_key = StreamManagerInner::media_key_for(&stream_key);
            self.inner
                .publish_media_event(MediaEvent::StreamOnlineChanged(StreamOnlineChanged {
                    header: crate::media_provider::util::event_header(
                        "stream-manager",
                        Some(&media_key),
                        None,
                    ),
                    online: OnlineState::Online,
                    schema: None,
                }));
        }
        let lease = PublishLease {
            stream_id: entry.stream_id,
            stream_key: stream_key.clone(),
            lease_id,
        };
        let sink = PublisherHandle {
            inner: self.inner.clone(),
            stream_key: stream_key.clone(),
            entry,
            lease_id,
            closed: AtomicBool::new(false),
        };
        Ok((lease, Box::new(sink)))
    }

    async fn release_publisher(&self, lease: &PublishLease) -> Result<(), SdkError> {
        self.inner.release_lease(&lease.stream_key, lease.lease_id)
    }
}

/// `SubscriberApi` implementation: subscribe, bootstrap, and manage backpressure.
///
/// `SubscriberApi` 实现：订阅、引导和管理背压。
#[async_trait]
impl SubscriberApi for StreamManager {
    async fn subscribe(
        &self,
        stream_key: StreamKey,
        options: SubscriberOptions,
    ) -> Result<Box<dyn SubscriberSource>, SdkError> {
        if options.queue_capacity == 0 {
            return Err(SdkError::InvalidArgument(
                "subscriber queue_capacity must be greater than zero".to_string(),
            ));
        }
        if options.bootstrap_policy.max_bootstrap_frames > options.queue_capacity {
            return Err(SdkError::InvalidArgument(format!(
                "subscriber queue_capacity ({}) must be >= bootstrap max frames ({})",
                options.queue_capacity, options.bootstrap_policy.max_bootstrap_frames
            )));
        }

        let entry = self
            .inner
            .streams
            .get(&stream_key)
            .map(|v| Arc::clone(v.value()))
            .ok_or_else(|| SdkError::NotFound(format!("stream {stream_key}")))?;

        let (subscriber, rx) = entry.dispatcher.add_subscriber(&options);
        self.inner.publish_stream_event(
            &stream_key,
            StreamEventKind::SubscriberOpened,
            Some(entry.stream_id),
            Some(subscriber.id),
            None,
            None,
        );

        for frame in entry.ring.bootstrap_frames(options.bootstrap_policy) {
            if subscriber.tx.try_send(frame).is_err() {
                break;
            }
        }

        Ok(Box::new(SubscriberHandle {
            inner: self.inner.clone(),
            stream_key,
            stream_id: entry.stream_id,
            id: subscriber.id,
            rx,
            dispatcher: entry.dispatcher.clone(),
            closed: false,
        }))
    }
}

/// `StreamManagerApi` implementation: snapshots, keyframe requests, and idle cleanup.
///
/// `StreamManagerApi` 实现：快照、关键帧请求与空闲清理。
#[async_trait]
impl StreamManagerApi for StreamManager {
    async fn open_publisher(
        &self,
        stream_key: StreamKey,
        options: PublisherOptions,
    ) -> Result<Box<dyn PublisherSink>, SdkError> {
        let _ = self.inner.mode;
        let (_, sink) = self.acquire_publisher(stream_key, options).await?;
        Ok(sink)
    }

    async fn open_subscriber(
        &self,
        stream_key: StreamKey,
        options: SubscriberOptions,
    ) -> Result<Box<dyn SubscriberSource>, SdkError> {
        self.subscribe(stream_key, options).await
    }

    async fn list_streams(&self) -> Result<Vec<StreamSnapshot>, SdkError> {
        let out = self
            .inner
            .streams
            .iter()
            .map(|entry| self.inner.snapshot_for(entry.key(), entry.value()))
            .collect();
        Ok(out)
    }

    async fn get_stream(&self, stream_key: &StreamKey) -> Result<Option<StreamSnapshot>, SdkError> {
        let snapshot = self
            .inner
            .streams
            .get(stream_key)
            .map(|entry| self.inner.snapshot_for(entry.key(), entry.value()));
        Ok(snapshot)
    }

    async fn request_keyframe(&self, stream_key: &StreamKey) -> Result<(), SdkError> {
        let entry = self
            .inner
            .streams
            .get(stream_key)
            .ok_or_else(|| SdkError::NotFound(format!("stream {stream_key} not found")))?;
        entry
            .value()
            .keyframe_requests
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn close_idle_publishers(&self, max_idle_secs: u64) -> Result<usize, SdkError> {
        if max_idle_secs == 0 {
            return Ok(0);
        }
        let now_micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        let threshold_micros = max_idle_secs.saturating_mul(1_000_000);
        let mut closed = 0usize;

        for entry in self.inner.streams.iter() {
            let val = entry.value();
            // Only consider streams with an active publisher and no subscribers
            if val.active_lease.load(Ordering::Acquire) == 0 {
                continue;
            }
            if val.dispatcher.subscriber_count() > 0 {
                // Has subscribers — reset idle timer
                val.last_no_subscriber_micros.store(0, Ordering::Relaxed);
                continue;
            }
            let last_empty = val.last_no_subscriber_micros.load(Ordering::Relaxed);
            if last_empty == 0 {
                continue;
            }
            if now_micros.saturating_sub(last_empty) >= threshold_micros {
                // Idle too long — release the publisher lease
                let lease_id = val.active_lease.load(Ordering::Acquire);
                if lease_id != 0 {
                    let _ = self.inner.release_lease(entry.key(), lease_id);
                    closed += 1;
                }
            }
        }
        Ok(closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use cheetah_codec::{CodecId, FrameFormat, MediaKind, Timebase, TrackId, TrackInfo};
    use cheetah_media_api::event::{MediaEvent, MediaEventBusApi, MediaEventSender};
    use cheetah_runtime_tokio::TokioRuntime;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
    use tokio::time::{timeout, Duration};

    use crate::media_provider::LocalMediaEventBus;

    fn make_frame(payload: &'static [u8], key: bool) -> Arc<AVFrame> {
        make_frame_at(payload, key, 0)
    }

    fn make_frame_at(payload: &'static [u8], key: bool, ts_ms: i64) -> Arc<AVFrame> {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            ts_ms,
            ts_ms,
            Timebase::new(1, 1000),
            Bytes::from_static(payload),
        );
        if key {
            frame.flags.insert(FrameFlags::KEY);
        }
        Arc::new(frame)
    }

    fn make_discontinuity_frame(payload: &'static [u8], key: bool, ts_ms: i64) -> Arc<AVFrame> {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            ts_ms,
            ts_ms,
            Timebase::new(1, 1000),
            Bytes::from_static(payload),
        );
        if key {
            frame.flags.insert(FrameFlags::KEY);
        }
        frame.flags.insert(FrameFlags::DISCONTINUITY);
        Arc::new(frame)
    }

    fn make_audio_frame_at(payload: &'static [u8], ts_ms: i64) -> Arc<AVFrame> {
        Arc::new(AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::DataPacket,
            ts_ms,
            ts_ms,
            Timebase::new(1, 1000),
            Bytes::from_static(payload),
        ))
    }

    #[test]
    fn ring_buffer_write_and_bootstrap() {
        let ring = RingBuffer::new(8);
        let frame = make_frame(b"x", true);

        ring.push(frame.clone());
        let list = ring.bootstrap_frames(BootstrapPolicy {
            mode: BootstrapMode::FullGop,
            max_bootstrap_age_ms: None,
            max_bootstrap_frames: 4,
            wait_for_next_random_access_point: false,
        });
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].payload, frame.payload);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shared_pool_mode_dispatches() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::SharedPool { workers: 2 }, 128, runtime);
        let key = StreamKey::new("live", "demo");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");
        let mut subscriber = manager
            .open_subscriber(key, SubscriberOptions::default())
            .await
            .expect("subscriber");

        let frame = make_frame(b"f1", true);
        let _ = publisher.push_frame(frame).expect("dispatch");
        let got = subscriber.recv().await.expect("recv").expect("frame");
        assert_eq!(got.payload, Bytes::from_static(b"f1"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_snapshot_reports_publisher_and_tracks() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 128, runtime);
        let key = StreamKey::new("live", "snapshot");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        publisher
            .update_tracks(vec![TrackInfo::new(
                TrackId(1),
                MediaKind::Video,
                CodecId::H264,
                90_000,
            )])
            .expect("update tracks");

        let snapshot = manager
            .get_stream(&key)
            .await
            .expect("snapshot")
            .expect("stream exists");
        assert!(snapshot.publisher_active);
        assert_eq!(snapshot.subscriber_count, 0);
        assert_eq!(snapshot.tracks.len(), 1);

        publisher.close().expect("close");
        let after_close = manager.get_stream(&key).await.expect("snapshot");
        assert!(after_close.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rejects_second_publisher_on_same_stream() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 128, runtime);
        let key = StreamKey::new("live", "exclusive");
        let _first = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("first publisher");
        let second = manager
            .open_publisher(key, PublisherOptions::default())
            .await;
        assert!(matches!(second, Err(SdkError::Conflict(_))));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bootstrap_respects_max_frames() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 128, runtime);
        let key = StreamKey::new("live", "bootstrap");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        for _ in 0..8u8 {
            let frame = make_frame(b"x", true);
            let _ = publisher.push_frame(frame).expect("dispatch");
        }

        let mut subscriber = manager
            .open_subscriber(
                key,
                SubscriberOptions {
                    queue_capacity: 8,
                    backpressure: BackpressurePolicy::DropDroppableFirst,
                    bootstrap_policy: BootstrapPolicy::full_gop(3, None),
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("subscriber");

        let f1 = timeout(Duration::from_millis(20), subscriber.recv())
            .await
            .expect("frame 1 timeout")
            .expect("recv result")
            .expect("frame 1");
        let f2 = timeout(Duration::from_millis(20), subscriber.recv())
            .await
            .expect("frame 2 timeout")
            .expect("recv result")
            .expect("frame 2");
        let f3 = timeout(Duration::from_millis(20), subscriber.recv())
            .await
            .expect("frame 3 timeout")
            .expect("recv result")
            .expect("frame 3");
        assert_eq!(f1.payload.len(), 1);
        assert_eq!(f2.payload.len(), 1);
        assert_eq!(f3.payload.len(), 1);

        let fourth = timeout(Duration::from_millis(20), subscriber.recv()).await;
        assert!(fourth.is_err(), "must not bootstrap more than max frames");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn keyframe_bootstrap_returns_empty_without_keyframe_in_window() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 8, runtime);
        let key = StreamKey::new("live", "no-key");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        for _ in 0..6u8 {
            let frame = make_frame(b"x", false);
            let _ = publisher.push_frame(frame).expect("dispatch");
        }

        let mut subscriber = manager
            .open_subscriber(
                key,
                SubscriberOptions {
                    queue_capacity: 8,
                    backpressure: BackpressurePolicy::DropDroppableFirst,
                    bootstrap_policy: BootstrapPolicy {
                        mode: BootstrapMode::LiveTail,
                        max_bootstrap_age_ms: None,
                        max_bootstrap_frames: 4,
                        wait_for_next_random_access_point: true,
                    },
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("subscriber");

        let result = timeout(Duration::from_millis(20), subscriber.recv()).await;
        assert!(
            result.is_err(),
            "bootstrap must be empty when no keyframe exists in window"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stale_keyframe_index_does_not_seed_bootstrap() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 8, runtime);
        let key = StreamKey::new("live", "stale-idr");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        let _ = publisher
            .push_frame(make_frame(b"k", true))
            .expect("dispatch key");
        for _ in 0..16u8 {
            let _ = publisher
                .push_frame(make_frame(b"x", false))
                .expect("dispatch non-key");
        }

        let mut subscriber = manager
            .open_subscriber(
                key,
                SubscriberOptions {
                    queue_capacity: 8,
                    backpressure: BackpressurePolicy::DropDroppableFirst,
                    bootstrap_policy: BootstrapPolicy {
                        mode: BootstrapMode::LiveTail,
                        max_bootstrap_age_ms: None,
                        max_bootstrap_frames: 8,
                        wait_for_next_random_access_point: true,
                    },
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("subscriber");

        let result = timeout(Duration::from_millis(20), subscriber.recv()).await;
        assert!(
            result.is_err(),
            "stale keyframe index must not seed bootstrap after overwrite"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn disconnect_on_overflow_removes_subscriber() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 128, runtime);
        let key = StreamKey::new("live", "overflow");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");
        let _subscriber = manager
            .open_subscriber(
                key.clone(),
                SubscriberOptions {
                    queue_capacity: 1,
                    backpressure: BackpressurePolicy::DisconnectOnOverflow,
                    bootstrap_policy: BootstrapPolicy {
                        mode: BootstrapMode::LiveTail,
                        max_bootstrap_age_ms: None,
                        max_bootstrap_frames: 1,
                        wait_for_next_random_access_point: false,
                    },
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("subscriber");

        let _ = publisher
            .push_frame(make_frame(b"a", false))
            .expect("dispatch 1");
        let _ = publisher
            .push_frame(make_frame(b"b", false))
            .expect("dispatch 2");
        let _ = publisher
            .push_frame(make_frame(b"c", false))
            .expect("dispatch 3");

        let snapshot = manager
            .get_stream(&key)
            .await
            .expect("snapshot")
            .expect("stream");
        assert_eq!(
            snapshot.subscriber_count, 0,
            "overflow policy disconnect should remove subscriber"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bootstrap_live_tail_respects_max_age_ms() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 128, runtime);
        let key = StreamKey::new("live", "age-limit");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        let _ = publisher
            .push_frame(make_frame_at(b"a", true, 0))
            .expect("dispatch");
        let _ = publisher
            .push_frame(make_frame_at(b"b", true, 1_000))
            .expect("dispatch");
        let _ = publisher
            .push_frame(make_frame_at(b"c", true, 2_000))
            .expect("dispatch");
        let _ = publisher
            .push_frame(make_frame_at(b"d", true, 3_000))
            .expect("dispatch");

        let mut subscriber = manager
            .open_subscriber(
                key,
                SubscriberOptions {
                    queue_capacity: 8,
                    backpressure: BackpressurePolicy::DropDroppableFirst,
                    bootstrap_policy: BootstrapPolicy {
                        mode: BootstrapMode::LiveTail,
                        max_bootstrap_age_ms: Some(500),
                        max_bootstrap_frames: 8,
                        wait_for_next_random_access_point: true,
                    },
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("subscriber");

        let frame = timeout(Duration::from_millis(20), subscriber.recv())
            .await
            .expect("frame timeout")
            .expect("recv result")
            .expect("frame");
        assert_eq!(frame.payload, Bytes::from_static(b"d"));
        let second = timeout(Duration::from_millis(20), subscriber.recv()).await;
        assert!(
            second.is_err(),
            "max_bootstrap_age_ms should trim old frames"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bootstrap_can_fallback_without_random_access_when_wait_disabled() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 8, runtime);
        let key = StreamKey::new("live", "fallback-no-idr");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        let _ = publisher
            .push_frame(make_frame_at(b"x", false, 0))
            .expect("dispatch");
        let _ = publisher
            .push_frame(make_frame_at(b"y", false, 40))
            .expect("dispatch");

        let mut subscriber = manager
            .open_subscriber(
                key,
                SubscriberOptions {
                    queue_capacity: 8,
                    backpressure: BackpressurePolicy::DropDroppableFirst,
                    bootstrap_policy: BootstrapPolicy {
                        mode: BootstrapMode::LiveTail,
                        max_bootstrap_age_ms: None,
                        max_bootstrap_frames: 2,
                        wait_for_next_random_access_point: false,
                    },
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("subscriber");

        let first = timeout(Duration::from_millis(20), subscriber.recv())
            .await
            .expect("first timeout")
            .expect("recv result")
            .expect("first frame");
        assert_eq!(first.payload, Bytes::from_static(b"x"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bootstrap_waits_for_next_keyframe_after_discontinuity() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 16, runtime);
        let key = StreamKey::new("live", "discontinuity-wait-keyframe");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        let _ = publisher
            .push_frame(make_frame_at(b"k0", true, 0))
            .expect("dispatch keyframe");
        let _ = publisher
            .push_frame(make_frame_at(b"p0", false, 40))
            .expect("dispatch p-frame");
        let _ = publisher
            .push_frame(make_discontinuity_frame(b"r0", false, 0))
            .expect("dispatch discontinuity");
        let _ = publisher
            .push_frame(make_frame_at(b"p1", false, 40))
            .expect("dispatch p-frame after reset");

        let mut subscriber = manager
            .open_subscriber(
                key,
                SubscriberOptions {
                    queue_capacity: 8,
                    backpressure: BackpressurePolicy::DropDroppableFirst,
                    bootstrap_policy: BootstrapPolicy {
                        mode: BootstrapMode::LiveTail,
                        max_bootstrap_age_ms: None,
                        max_bootstrap_frames: 8,
                        wait_for_next_random_access_point: true,
                    },
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("subscriber");

        let result = timeout(Duration::from_millis(20), subscriber.recv()).await;
        assert!(
            result.is_err(),
            "bootstrap should wait for a new random access point after discontinuity"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bootstrap_uses_cached_keyframe_when_reordered_frames_are_not_discontinuities() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 16, runtime);
        let key = StreamKey::new("live", "reordered-bframe-bootstrap");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        let _ = publisher
            .push_frame(make_frame_at(b"k0", true, 0))
            .expect("dispatch keyframe");
        let _ = publisher
            .push_frame(make_frame_at(b"p0", false, 40))
            .expect("dispatch p-frame");
        let _ = publisher
            .push_frame(make_frame_at(b"b0", false, 41))
            .expect("dispatch repaired b-frame");

        let mut subscriber = manager
            .open_subscriber(
                key,
                SubscriberOptions {
                    queue_capacity: 8,
                    backpressure: BackpressurePolicy::DropDroppableFirst,
                    bootstrap_policy: BootstrapPolicy {
                        mode: BootstrapMode::LiveTail,
                        max_bootstrap_age_ms: None,
                        max_bootstrap_frames: 8,
                        wait_for_next_random_access_point: true,
                    },
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("subscriber");

        let first = timeout(Duration::from_millis(20), subscriber.recv())
            .await
            .expect("bootstrap frame timeout")
            .expect("recv result")
            .expect("frame");
        assert_eq!(first.payload, Bytes::from_static(b"k0"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bootstrap_fallback_does_not_cross_discontinuity_boundary() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 16, runtime);
        let key = StreamKey::new("live", "discontinuity-fallback-boundary");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        let _ = publisher
            .push_frame(make_frame_at(b"k0", true, 0))
            .expect("dispatch keyframe");
        let _ = publisher
            .push_frame(make_frame_at(b"p0", false, 40))
            .expect("dispatch p-frame");
        let _ = publisher
            .push_frame(make_discontinuity_frame(b"r0", false, 0))
            .expect("dispatch discontinuity");
        let _ = publisher
            .push_frame(make_frame_at(b"p1", false, 40))
            .expect("dispatch p-frame after reset");

        let mut subscriber = manager
            .open_subscriber(
                key,
                SubscriberOptions {
                    queue_capacity: 8,
                    backpressure: BackpressurePolicy::DropDroppableFirst,
                    bootstrap_policy: BootstrapPolicy {
                        mode: BootstrapMode::LiveTail,
                        max_bootstrap_age_ms: None,
                        max_bootstrap_frames: 8,
                        wait_for_next_random_access_point: false,
                    },
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("subscriber");

        let first = timeout(Duration::from_millis(20), subscriber.recv())
            .await
            .expect("first timeout")
            .expect("recv result")
            .expect("first frame");
        assert_eq!(
            first.payload,
            Bytes::from_static(b"r0"),
            "bootstrap fallback should begin at discontinuity boundary"
        );

        let second = timeout(Duration::from_millis(20), subscriber.recv())
            .await
            .expect("second timeout")
            .expect("recv result")
            .expect("second frame");
        assert_eq!(second.payload, Bytes::from_static(b"p1"));

        let third = timeout(Duration::from_millis(20), subscriber.recv()).await;
        assert!(
            third.is_err(),
            "bootstrap fallback should not include pre-discontinuity frames"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscribe_rejects_queue_capacity_smaller_than_bootstrap_window() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 128, runtime);
        let key = StreamKey::new("live", "invalid-subscriber-options");
        let _publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        let err = match manager
            .open_subscriber(
                key,
                SubscriberOptions {
                    queue_capacity: 2,
                    backpressure: BackpressurePolicy::DropDroppableFirst,
                    bootstrap_policy: BootstrapPolicy::full_gop(4, None),
                    media_filter: MediaFilter::default(),
                },
            )
            .await
        {
            Ok(_) => panic!("queue smaller than bootstrap window must be rejected"),
            Err(err) => err,
        };

        match err {
            SdkError::InvalidArgument(msg) => {
                assert!(
                    msg.contains("queue_capacity") && msg.contains("bootstrap max frames"),
                    "unexpected invalid argument message: {msg}"
                );
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn slow_subscriber_does_not_block_fast_subscriber_delivery() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 256, runtime);
        let key = StreamKey::new("live", "slow-subscriber-isolation");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        let _slow_subscriber = manager
            .open_subscriber(
                key.clone(),
                SubscriberOptions {
                    queue_capacity: 1,
                    backpressure: BackpressurePolicy::DropUntilNextKeyframe,
                    bootstrap_policy: BootstrapPolicy::none(),
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("slow subscriber");

        let mut fast_subscriber = manager
            .open_subscriber(
                key.clone(),
                SubscriberOptions {
                    queue_capacity: 64,
                    backpressure: BackpressurePolicy::DropDroppableFirst,
                    bootstrap_policy: BootstrapPolicy::none(),
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("fast subscriber");

        for idx in 0..16 {
            let video = make_frame_at(b"v", idx * 33 == 0, idx * 33);
            let result = publisher.push_frame(video).expect("dispatch video");
            assert_eq!(
                result,
                DispatchResult::Accepted,
                "slow subscriber backpressure must not downgrade publisher dispatch result"
            );

            if idx % 6 == 0 {
                let audio = make_audio_frame_at(b"a", idx * 33);
                let audio_result = publisher.push_frame(audio).expect("dispatch audio");
                assert_eq!(
                    audio_result,
                    DispatchResult::Accepted,
                    "audio dispatch should remain accepted with fast subscriber attached"
                );
            }
        }

        let mut received = 0usize;
        while received < 19 {
            let frame = timeout(Duration::from_millis(50), fast_subscriber.recv())
                .await
                .expect("fast subscriber timeout")
                .expect("fast subscriber recv result")
                .expect("fast subscriber frame");
            assert!(
                frame.payload == Bytes::from_static(b"v")
                    || frame.payload == Bytes::from_static(b"a")
            );
            received += 1;
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconnect_bootstrap_respects_discontinuity_bounds_for_mixed_av_backlog() {
        let runtime = Arc::new(TokioRuntime::new());
        let manager = StreamManager::new(DispatcherMode::PerStream, 4096, runtime);
        let key = StreamKey::new("live", "mixed-av-reconnect-bootstrap");
        let publisher = manager
            .open_publisher(key.clone(), PublisherOptions::default())
            .await
            .expect("publisher");

        for idx in 0..180 {
            let ts = idx * 33;
            let is_key = idx == 0 || idx == 120;
            let _ = publisher
                .push_frame(make_frame_at(b"old", is_key, ts))
                .expect("dispatch old video");
            if idx % 6 == 0 {
                let _ = publisher
                    .push_frame(make_audio_frame_at(b"old", ts))
                    .expect("dispatch old audio");
            }
        }

        let _ = publisher
            .push_frame(make_discontinuity_frame(b"new", true, 0))
            .expect("dispatch discontinuity keyframe");
        for idx in 1..20 {
            let ts = idx * 33;
            let _ = publisher
                .push_frame(make_frame_at(b"new", false, ts))
                .expect("dispatch new video");
            if idx % 5 == 0 {
                let _ = publisher
                    .push_frame(make_audio_frame_at(b"new", ts))
                    .expect("dispatch new audio");
            }
        }

        let mut subscriber = manager
            .open_subscriber(
                key,
                SubscriberOptions {
                    queue_capacity: 64,
                    backpressure: BackpressurePolicy::DropDroppableFirst,
                    bootstrap_policy: BootstrapPolicy {
                        mode: BootstrapMode::LiveTail,
                        max_bootstrap_age_ms: None,
                        max_bootstrap_frames: 32,
                        wait_for_next_random_access_point: true,
                    },
                    media_filter: MediaFilter::default(),
                },
            )
            .await
            .expect("subscriber");

        let mut frames = Vec::new();
        loop {
            match timeout(Duration::from_millis(20), subscriber.recv()).await {
                Ok(Ok(Some(frame))) => frames.push(frame),
                Ok(Ok(None)) | Ok(Err(_)) | Err(_) => break,
            }
            if frames.len() >= 64 {
                break;
            }
        }

        assert!(
            !frames.is_empty(),
            "reconnect bootstrap should provide post-discontinuity frames"
        );
        assert!(
            frames.len() <= 32,
            "bootstrap must not exceed configured max frame count"
        );
        assert!(
            frames
                .iter()
                .all(|frame| frame.payload == Bytes::from_static(b"new")),
            "bootstrap frames must not include pre-discontinuity backlog"
        );
    }

    struct CollectingMediaEventSender(UnboundedSender<MediaEvent>);

    impl MediaEventSender for CollectingMediaEventSender {
        fn send(&self, event: MediaEvent) -> cheetah_media_api::error::Result<()> {
            let _ = self.0.send(event);
            Ok(())
        }

        fn lagged(&self, _dropped: u64) -> cheetah_media_api::error::Result<()> {
            Ok(())
        }
    }

    async fn recv_event(
        rx: &mut UnboundedReceiver<MediaEvent>,
        deadline: Duration,
    ) -> Option<MediaEvent> {
        timeout(deadline, rx.recv()).await.ok().flatten()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn media_event_stream_lifecycle() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let bus = Arc::new(LocalMediaEventBus::new(runtime.clone()));
        let (tx, mut rx) = unbounded_channel();
        let _sub = bus
            .subscribe(Box::new(CollectingMediaEventSender(tx)), 64)
            .unwrap();

        let manager = StreamManager::new(DispatcherMode::PerStream, 128, runtime);
        manager.set_media_event_bus(bus);

        let key = StreamKey::new("live", "media-event");
        let options = PublisherOptions {
            announce_tracks: true,
            protocol: "rtmp".to_string(),
            remote_endpoint: Some("1.2.3.4:1935".to_string()),
        };
        let (_lease, sink) = manager
            .acquire_publisher(key.clone(), options)
            .await
            .expect("publisher");

        let first = recv_event(&mut rx, Duration::from_millis(100))
            .await
            .expect("online event after acquire");
        assert!(matches!(first, MediaEvent::StreamOnlineChanged(_)));

        let track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90000);
        sink.update_tracks(vec![track]).unwrap();

        let second = recv_event(&mut rx, Duration::from_millis(100))
            .await
            .expect("published event after tracks");
        match second {
            MediaEvent::StreamPublished(p) => {
                assert_eq!(p.protocol, "rtmp");
                assert_eq!(p.remote_endpoint.as_deref(), Some("1.2.3.4:1935"));
            }
            _ => panic!("expected StreamPublished, got {second:?}"),
        }

        sink.close().unwrap();

        let third = recv_event(&mut rx, Duration::from_millis(100))
            .await
            .expect("unpublished event after close");
        assert!(matches!(third, MediaEvent::StreamUnpublished(_)));

        let fourth = recv_event(&mut rx, Duration::from_millis(100))
            .await
            .expect("offline event after close");
        assert!(matches!(fourth, MediaEvent::StreamOnlineChanged(_)));
    }
}
