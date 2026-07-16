use std::sync::{Arc, RwLock};

use crate::idempotency::InMemoryIdempotencyRepository;
use crate::output::OutputRegistryRegistration;
use cheetah_media_api::capability::{MediaCapabilityDescriptor, MediaCapabilityReport};
use cheetah_media_api::image::ImageEncodeApi;
use cheetah_media_api::port::{
    MediaControlApi, MediaOutputRegistryApi, ProxyApi, PublishSubscribeApi, RecordApi, RtpApi,
    SnapshotApi, WebhookApi,
};
use cheetah_media_api::{MediaCapability, MediaCapabilitySet};

/// A registration handle returned when a provider is registered with
/// `MediaServices`. It can be used to unregister the provider safely across
/// restarts and concurrent replacements.
///
/// `MediaServices` 注册 provider 后返回的句柄，可用于安全地跨重启或并发替换注销 provider。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRegistration {
    pub capability: MediaCapability,
    pub provider_id: String,
    pub generation: u64,
}

/// Mutable registry of media capability providers.
///
/// Each provider is stored as an `Arc<dyn ...>` and can be replaced at runtime,
/// allowing feature modules to register their implementations after they are
/// initialized. The registry is shared across all clones of `MediaServices`.
///
/// 媒体能力 provider 的可变注册表。
///
/// 每个 provider 以 `Arc<dyn ...>` 形式保存，可在运行时被替换，允许特性模块
/// 在初始化后注册各自的实现。该注册表在所有 `MediaServices` 克隆之间共享。
#[derive(Clone)]
pub struct MediaServices {
    inner: Arc<RwLock<MediaProviderRegistry>>,
    idempotency: Arc<InMemoryIdempotencyRepository>,
}

impl MediaServices {
    /// Create a media services bundle with all capabilities unavailable.
    ///
    /// 创建所有能力均不可用的 media services 束。
    pub fn unavailable() -> Self {
        Self {
            inner: Arc::new(RwLock::new(MediaProviderRegistry::empty())),
            idempotency: Arc::new(InMemoryIdempotencyRepository::new()),
        }
    }

    /// Return the shared idempotency repository.
    ///
    /// 返回共享的幂等仓库。
    pub fn idempotency(&self) -> Arc<InMemoryIdempotencyRepository> {
        self.idempotency.clone()
    }

    /// Register the output endpoint registry.
    ///
    /// 注册输出端点注册表。
    pub fn register_output_registry(
        &self,
        registry: Arc<dyn MediaOutputRegistryApi>,
    ) -> OutputRegistryRegistration {
        let mut inner = self.inner.write().expect("media services lock");
        inner.generation += 1;
        let generation = inner.generation;
        let provider_id = format!("output_registry:{generation}");
        inner.output_registry = Some(OutputRegistrySlot {
            registry,
            generation,
        });
        OutputRegistryRegistration {
            provider_id,
            generation,
        }
    }

    /// Return the current output registry, if any.
    ///
    /// 返回当前输出注册表（如有）。
    pub fn output_registry(&self) -> Option<Arc<dyn MediaOutputRegistryApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .output_registry
            .as_ref()
            .map(|s| s.registry.clone())
    }

    /// Unregister the output registry using a previously returned registration.
    ///
    /// 使用之前返回的注册句柄注销输出注册表。
    pub fn unregister_output_registry(&self, registration: &OutputRegistryRegistration) -> bool {
        let mut inner = self.inner.write().expect("media services lock");
        if inner
            .output_registry
            .as_ref()
            .map(|s| s.generation)
            .is_some_and(|g| g == registration.generation)
        {
            inner.output_registry = None;
            inner.generation += 1;
            true
        } else {
            false
        }
    }

    /// Register the control provider.
    ///
    /// 注册控制 provider。
    pub fn register_control(&self, control: Arc<dyn MediaControlApi>) -> ProviderRegistration {
        self.register_control_with_capabilities(control, control_default_capabilities())
    }

    /// Register the control provider with explicit capabilities.
    ///
    /// 注册带显式能力声明的控制 provider。
    pub fn register_control_with_capabilities(
        &self,
        control: Arc<dyn MediaControlApi>,
        capabilities: MediaCapabilitySet,
    ) -> ProviderRegistration {
        let mut registry = self.inner.write().expect("media services lock");
        registry.generation += 1;
        let generation = registry.generation;
        let provider_id = format!("control:{generation}");
        let descriptors = descriptors_from_set(&capabilities, &provider_id);
        registry.control = Some(ProviderEntry {
            provider: control,
            generation,
            capabilities,
            descriptors,
        });
        ProviderRegistration {
            capability: MediaCapability::Query,
            provider_id: format!("control:{generation}"),
            generation,
        }
    }

    /// Return the current control provider, if any.
    ///
    /// 返回当前控制 provider（如有）。
    pub fn control(&self) -> Option<Arc<dyn MediaControlApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .control
            .as_ref()
            .map(|e| e.provider.clone())
    }

    /// Register the publish/subscribe provider.
    ///
    /// 注册发布/订阅 provider。
    pub fn register_publish_subscribe(
        &self,
        publish_subscribe: Arc<dyn PublishSubscribeApi>,
    ) -> ProviderRegistration {
        self.register_publish_subscribe_with_capabilities(
            publish_subscribe,
            publish_subscribe_default_capabilities(),
        )
    }

    /// Register the publish/subscribe provider with explicit capabilities.
    ///
    /// 注册带显式能力声明的发布/订阅 provider。
    pub fn register_publish_subscribe_with_capabilities(
        &self,
        publish_subscribe: Arc<dyn PublishSubscribeApi>,
        capabilities: MediaCapabilitySet,
    ) -> ProviderRegistration {
        let mut registry = self.inner.write().expect("media services lock");
        registry.generation += 1;
        let generation = registry.generation;
        let provider_id = format!("publish_subscribe:{generation}");
        let descriptors = descriptors_from_set(&capabilities, &provider_id);
        registry.publish_subscribe = Some(ProviderEntry {
            provider: publish_subscribe,
            generation,
            capabilities,
            descriptors,
        });
        ProviderRegistration {
            capability: MediaCapability::Publish,
            provider_id: format!("publish_subscribe:{generation}"),
            generation,
        }
    }

    /// Return the current publish/subscribe provider, if any.
    ///
    /// 返回当前发布/订阅 provider（如有）。
    pub fn publish_subscribe(&self) -> Option<Arc<dyn PublishSubscribeApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .publish_subscribe
            .as_ref()
            .map(|e| e.provider.clone())
    }

    /// Register the record provider.
    ///
    /// 注册录制 provider。
    pub fn register_record(&self, record: Arc<dyn RecordApi>) -> ProviderRegistration {
        self.register_record_with_capabilities(record, record_default_capabilities())
    }

    /// Register the record provider with explicit capabilities.
    ///
    /// 注册带显式能力声明的录制 provider。
    pub fn register_record_with_capabilities(
        &self,
        record: Arc<dyn RecordApi>,
        capabilities: MediaCapabilitySet,
    ) -> ProviderRegistration {
        let mut registry = self.inner.write().expect("media services lock");
        registry.generation += 1;
        let generation = registry.generation;
        let provider_id = format!("record:{generation}");
        let descriptors = descriptors_from_set(&capabilities, &provider_id);
        registry.record = Some(ProviderEntry {
            provider: record,
            generation,
            capabilities,
            descriptors,
        });
        ProviderRegistration {
            capability: MediaCapability::Record,
            provider_id: format!("record:{generation}"),
            generation,
        }
    }

    /// Return the current record provider, if any.
    ///
    /// 返回当前录制 provider（如有）。
    pub fn record(&self) -> Option<Arc<dyn RecordApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .record
            .as_ref()
            .map(|e| e.provider.clone())
    }

    /// Register the snapshot provider.
    ///
    /// 注册快照 provider。
    pub fn register_snapshot(&self, snapshot: Arc<dyn SnapshotApi>) -> ProviderRegistration {
        self.register_snapshot_with_capabilities(snapshot, snapshot_default_capabilities())
    }

    /// Register the snapshot provider with explicit capabilities.
    ///
    /// 注册带显式能力声明的快照 provider。
    pub fn register_snapshot_with_capabilities(
        &self,
        snapshot: Arc<dyn SnapshotApi>,
        capabilities: MediaCapabilitySet,
    ) -> ProviderRegistration {
        let mut registry = self.inner.write().expect("media services lock");
        registry.generation += 1;
        let generation = registry.generation;
        let provider_id = format!("snapshot:{generation}");
        let descriptors = descriptors_from_set(&capabilities, &provider_id);
        registry.snapshot = Some(ProviderEntry {
            provider: snapshot,
            generation,
            capabilities,
            descriptors,
        });
        ProviderRegistration {
            capability: MediaCapability::Snapshot,
            provider_id: format!("snapshot:{generation}"),
            generation,
        }
    }

    /// Return the current snapshot provider, if any.
    ///
    /// 返回当前快照 provider（如有）。
    pub fn snapshot(&self) -> Option<Arc<dyn SnapshotApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .snapshot
            .as_ref()
            .map(|e| e.provider.clone())
    }

    /// Register the image encode provider.
    ///
    /// 注册图片编码 provider。
    pub fn register_image_encode(
        &self,
        image_encode: Arc<dyn ImageEncodeApi>,
    ) -> ProviderRegistration {
        let mut registry = self.inner.write().expect("media services lock");
        registry.generation += 1;
        let generation = registry.generation;
        let capabilities = image_encode_default_capabilities();
        let provider_id = format!("image_encode:{generation}");
        let descriptors = descriptors_from_set(&capabilities, &provider_id);
        registry.image_encode = Some(ProviderEntry {
            provider: image_encode,
            generation,
            capabilities,
            descriptors,
        });
        ProviderRegistration {
            capability: MediaCapability::ImageEncode,
            provider_id: format!("image_encode:{generation}"),
            generation,
        }
    }

    /// Return the current image encode provider, if any.
    ///
    /// 返回当前图片编码 provider（如有）。
    pub fn image_encode(&self) -> Option<Arc<dyn ImageEncodeApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .image_encode
            .as_ref()
            .map(|e| e.provider.clone())
    }

    /// Register the proxy provider.
    ///
    /// 注册代理 provider。
    pub fn register_proxy(&self, proxy: Arc<dyn ProxyApi>) -> ProviderRegistration {
        self.register_proxy_with_capabilities(proxy, proxy_default_capabilities())
    }

    /// Register the proxy provider with explicit capabilities.
    ///
    /// 注册带显式能力声明的代理 provider。
    pub fn register_proxy_with_capabilities(
        &self,
        proxy: Arc<dyn ProxyApi>,
        capabilities: MediaCapabilitySet,
    ) -> ProviderRegistration {
        let mut registry = self.inner.write().expect("media services lock");
        registry.generation += 1;
        let generation = registry.generation;
        let provider_id = format!("proxy:{generation}");
        let descriptors = descriptors_from_set(&capabilities, &provider_id);
        registry.proxy = Some(ProviderEntry {
            provider: proxy,
            generation,
            capabilities,
            descriptors,
        });
        ProviderRegistration {
            capability: MediaCapability::Proxy,
            provider_id: format!("proxy:{generation}"),
            generation,
        }
    }

    /// Return the current proxy provider, if any.
    ///
    /// 返回当前代理 provider（如有）。
    pub fn proxy(&self) -> Option<Arc<dyn ProxyApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .proxy
            .as_ref()
            .map(|e| e.provider.clone())
    }

    /// Register the RTP provider.
    ///
    /// 注册 RTP provider。
    pub fn register_rtp(&self, rtp: Arc<dyn RtpApi>) -> ProviderRegistration {
        self.register_rtp_with_capabilities(rtp, rtp_default_capabilities())
    }

    /// Register the RTP provider with explicit capabilities.
    ///
    /// 注册带显式能力声明的 RTP provider。
    pub fn register_rtp_with_capabilities(
        &self,
        rtp: Arc<dyn RtpApi>,
        capabilities: MediaCapabilitySet,
    ) -> ProviderRegistration {
        let mut registry = self.inner.write().expect("media services lock");
        registry.generation += 1;
        let generation = registry.generation;
        let provider_id = format!("rtp:{generation}");
        let descriptors = descriptors_from_set(&capabilities, &provider_id);
        registry.rtp = Some(ProviderEntry {
            provider: rtp,
            generation,
            capabilities,
            descriptors,
        });
        ProviderRegistration {
            capability: MediaCapability::Rtp,
            provider_id: format!("rtp:{generation}"),
            generation,
        }
    }

    /// Return the current RTP provider, if any.
    ///
    /// 返回当前 RTP provider（如有）。
    pub fn rtp(&self) -> Option<Arc<dyn RtpApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .rtp
            .as_ref()
            .map(|e| e.provider.clone())
    }

    /// Register the webhook provider.
    ///
    /// 注册 webhook provider。
    pub fn register_webhook(&self, webhook: Arc<dyn WebhookApi>) -> ProviderRegistration {
        let mut registry = self.inner.write().expect("media services lock");
        registry.generation += 1;
        let generation = registry.generation;
        let capabilities = webhook_default_capabilities();
        let provider_id = format!("webhook:{generation}");
        let descriptors = descriptors_from_set(&capabilities, &provider_id);
        registry.webhook = Some(ProviderEntry {
            provider: webhook,
            generation,
            capabilities,
            descriptors,
        });
        ProviderRegistration {
            capability: MediaCapability::Webhook,
            provider_id: format!("webhook:{generation}"),
            generation,
        }
    }

    /// Return the current webhook provider, if any.
    ///
    /// 返回当前 webhook provider（如有）。
    pub fn webhook(&self) -> Option<Arc<dyn WebhookApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .webhook
            .as_ref()
            .map(|e| e.provider.clone())
    }

    /// Unregister a provider using a previously returned `ProviderRegistration`.
    /// Returns `true` if the registration matched and the provider was removed.
    ///
    /// 使用之前返回的 `ProviderRegistration` 注销 provider。若 generation 匹配且成功移除则返回 `true`。
    pub fn unregister(&self, registration: &ProviderRegistration) -> bool {
        let mut registry = self.inner.write().expect("media services lock");
        let mut slot = match registry.slot_for(registration.capability) {
            Some(slot) => slot,
            None => return false,
        };
        if slot.generation() != Some(registration.generation) {
            return false;
        }
        slot.take();
        registry.generation += 1;
        true
    }

    /// Return the current capability set advertised by the registry. The set is
    /// the union of all registered provider capabilities.
    ///
    /// 返回注册表当前宣告的能力集，即所有已注册 provider 能力集的并集。
    pub fn capabilities(&self) -> MediaCapabilitySet {
        let registry = self.inner.read().expect("media services lock");
        let mut set = MediaCapabilitySet::empty();
        if let Some(entry) = &registry.control {
            set.merge(&entry.capabilities);
        }
        if let Some(entry) = &registry.publish_subscribe {
            set.merge(&entry.capabilities);
        }
        if let Some(entry) = &registry.record {
            set.merge(&entry.capabilities);
        }
        if let Some(entry) = &registry.snapshot {
            set.merge(&entry.capabilities);
        }
        if let Some(entry) = &registry.image_encode {
            set.merge(&entry.capabilities);
        }
        if let Some(entry) = &registry.proxy {
            set.merge(&entry.capabilities);
        }
        if let Some(entry) = &registry.rtp {
            set.merge(&entry.capabilities);
        }
        if let Some(entry) = &registry.webhook {
            set.merge(&entry.capabilities);
        }
        set
    }

    /// Return an aggregate capability report with per-provider descriptors.
    ///
    /// 返回带每个 provider 描述符的聚合能力报告。
    pub fn capability_report(&self) -> MediaCapabilityReport {
        let registry = self.inner.read().expect("media services lock");
        let mut descriptors = Vec::new();
        if let Some(entry) = &registry.control {
            descriptors.extend(entry.descriptors.clone());
        }
        if let Some(entry) = &registry.publish_subscribe {
            descriptors.extend(entry.descriptors.clone());
        }
        if let Some(entry) = &registry.record {
            descriptors.extend(entry.descriptors.clone());
        }
        if let Some(entry) = &registry.snapshot {
            descriptors.extend(entry.descriptors.clone());
        }
        if let Some(entry) = &registry.image_encode {
            descriptors.extend(entry.descriptors.clone());
        }
        if let Some(entry) = &registry.proxy {
            descriptors.extend(entry.descriptors.clone());
        }
        if let Some(entry) = &registry.rtp {
            descriptors.extend(entry.descriptors.clone());
        }
        if let Some(entry) = &registry.webhook {
            descriptors.extend(entry.descriptors.clone());
        }
        descriptors.sort_by(|a, b| {
            a.capability
                .cmp(&b.capability)
                .then_with(|| a.provider_id.cmp(&b.provider_id))
        });
        MediaCapabilityReport {
            generation: registry.generation,
            descriptors,
        }
    }
}

#[derive(Default)]
struct MediaProviderRegistry {
    generation: u64,
    control: Option<ProviderEntry<Arc<dyn MediaControlApi>>>,
    publish_subscribe: Option<ProviderEntry<Arc<dyn PublishSubscribeApi>>>,
    record: Option<ProviderEntry<Arc<dyn RecordApi>>>,
    snapshot: Option<ProviderEntry<Arc<dyn SnapshotApi>>>,
    image_encode: Option<ProviderEntry<Arc<dyn ImageEncodeApi>>>,
    proxy: Option<ProviderEntry<Arc<dyn ProxyApi>>>,
    rtp: Option<ProviderEntry<Arc<dyn RtpApi>>>,
    webhook: Option<ProviderEntry<Arc<dyn WebhookApi>>>,
    output_registry: Option<OutputRegistrySlot>,
}

struct OutputRegistrySlot {
    registry: Arc<dyn MediaOutputRegistryApi>,
    generation: u64,
}

struct ProviderEntry<P> {
    provider: P,
    generation: u64,
    capabilities: MediaCapabilitySet,
    descriptors: Vec<MediaCapabilityDescriptor>,
}

enum ProviderSlot<'a> {
    Control(&'a mut Option<ProviderEntry<Arc<dyn MediaControlApi>>>),
    PublishSubscribe(&'a mut Option<ProviderEntry<Arc<dyn PublishSubscribeApi>>>),
    Record(&'a mut Option<ProviderEntry<Arc<dyn RecordApi>>>),
    Snapshot(&'a mut Option<ProviderEntry<Arc<dyn SnapshotApi>>>),
    ImageEncode(&'a mut Option<ProviderEntry<Arc<dyn ImageEncodeApi>>>),
    Proxy(&'a mut Option<ProviderEntry<Arc<dyn ProxyApi>>>),
    Rtp(&'a mut Option<ProviderEntry<Arc<dyn RtpApi>>>),
    Webhook(&'a mut Option<ProviderEntry<Arc<dyn WebhookApi>>>),
}

impl<'a> ProviderSlot<'a> {
    fn generation(&self) -> Option<u64> {
        match self {
            ProviderSlot::Control(opt) => opt.as_ref().map(|e| e.generation),
            ProviderSlot::PublishSubscribe(opt) => opt.as_ref().map(|e| e.generation),
            ProviderSlot::Record(opt) => opt.as_ref().map(|e| e.generation),
            ProviderSlot::Snapshot(opt) => opt.as_ref().map(|e| e.generation),
            ProviderSlot::ImageEncode(opt) => opt.as_ref().map(|e| e.generation),
            ProviderSlot::Proxy(opt) => opt.as_ref().map(|e| e.generation),
            ProviderSlot::Rtp(opt) => opt.as_ref().map(|e| e.generation),
            ProviderSlot::Webhook(opt) => opt.as_ref().map(|e| e.generation),
        }
    }

    fn take(&mut self) {
        match self {
            ProviderSlot::Control(opt) => **opt = None,
            ProviderSlot::PublishSubscribe(opt) => **opt = None,
            ProviderSlot::Record(opt) => **opt = None,
            ProviderSlot::Snapshot(opt) => **opt = None,
            ProviderSlot::ImageEncode(opt) => **opt = None,
            ProviderSlot::Proxy(opt) => **opt = None,
            ProviderSlot::Rtp(opt) => **opt = None,
            ProviderSlot::Webhook(opt) => **opt = None,
        }
    }
}

impl MediaProviderRegistry {
    fn empty() -> Self {
        Self::default()
    }

    fn slot_for(&mut self, capability: MediaCapability) -> Option<ProviderSlot<'_>> {
        match capability {
            MediaCapability::Query | MediaCapability::SessionControl => {
                Some(ProviderSlot::Control(&mut self.control))
            }
            MediaCapability::Publish | MediaCapability::Subscribe => {
                Some(ProviderSlot::PublishSubscribe(&mut self.publish_subscribe))
            }
            MediaCapability::Record | MediaCapability::Playback => {
                Some(ProviderSlot::Record(&mut self.record))
            }
            MediaCapability::Snapshot => Some(ProviderSlot::Snapshot(&mut self.snapshot)),
            MediaCapability::ImageEncode => Some(ProviderSlot::ImageEncode(&mut self.image_encode)),
            MediaCapability::Proxy => Some(ProviderSlot::Proxy(&mut self.proxy)),
            MediaCapability::Rtp => Some(ProviderSlot::Rtp(&mut self.rtp)),
            MediaCapability::Webhook => Some(ProviderSlot::Webhook(&mut self.webhook)),
        }
    }
}

fn descriptors_from_set(
    set: &MediaCapabilitySet,
    provider_id: impl Into<String>,
) -> Vec<MediaCapabilityDescriptor> {
    MediaCapabilityReport::from_capability_set(set, provider_id).descriptors
}

fn control_default_capabilities() -> MediaCapabilitySet {
    let mut set = MediaCapabilitySet::empty();
    set.add(MediaCapability::Query, 1);
    set.add(MediaCapability::SessionControl, 1);
    set
}

fn publish_subscribe_default_capabilities() -> MediaCapabilitySet {
    let mut set = MediaCapabilitySet::empty();
    set.add(MediaCapability::Publish, 1);
    set.add(MediaCapability::Subscribe, 1);
    set
}

fn record_default_capabilities() -> MediaCapabilitySet {
    let mut set = MediaCapabilitySet::empty();
    set.add(MediaCapability::Record, 1);
    set
}

fn snapshot_default_capabilities() -> MediaCapabilitySet {
    let mut set = MediaCapabilitySet::empty();
    set.add(MediaCapability::Snapshot, 1);
    set
}

fn image_encode_default_capabilities() -> MediaCapabilitySet {
    let mut set = MediaCapabilitySet::empty();
    set.add(MediaCapability::ImageEncode, 1);
    set
}

fn proxy_default_capabilities() -> MediaCapabilitySet {
    let mut set = MediaCapabilitySet::empty();
    set.add(MediaCapability::Proxy, 1);
    set
}

fn rtp_default_capabilities() -> MediaCapabilitySet {
    let mut set = MediaCapabilitySet::empty();
    set.add(MediaCapability::Rtp, 1);
    set
}

fn webhook_default_capabilities() -> MediaCapabilitySet {
    let mut set = MediaCapabilitySet::empty();
    set.add(MediaCapability::Webhook, 1);
    set
}
