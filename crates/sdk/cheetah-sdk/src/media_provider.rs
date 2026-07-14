use std::sync::{Arc, RwLock};

use cheetah_media_api::port::{
    MediaControlApi, PublishSubscribeApi, RecordApi, RtpApi, ServerAdminApi, SnapshotApi, WebRtcApi,
};

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
}

impl MediaServices {
    /// Create a media services bundle with all capabilities unavailable.
    ///
    /// 创建所有能力均不可用的 media services 束。
    pub fn unavailable() -> Self {
        Self {
            inner: Arc::new(RwLock::new(MediaProviderRegistry::empty())),
        }
    }

    /// Register the control provider.
    ///
    /// 注册控制 provider。
    pub fn register_control(&self, control: Arc<dyn MediaControlApi>) {
        self.inner.write().expect("media services lock").control = Some(control);
    }

    /// Return the current control provider, if any.
    ///
    /// 返回当前控制 provider（如有）。
    pub fn control(&self) -> Option<Arc<dyn MediaControlApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .control
            .clone()
    }

    /// Register the publish/subscribe provider.
    ///
    /// 注册发布/订阅 provider。
    pub fn register_publish_subscribe(&self, publish_subscribe: Arc<dyn PublishSubscribeApi>) {
        self.inner
            .write()
            .expect("media services lock")
            .publish_subscribe = Some(publish_subscribe);
    }

    /// Return the current publish/subscribe provider, if any.
    ///
    /// 返回当前发布/订阅 provider（如有）。
    pub fn publish_subscribe(&self) -> Option<Arc<dyn PublishSubscribeApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .publish_subscribe
            .clone()
    }

    /// Register the record provider.
    ///
    /// 注册录制 provider。
    pub fn register_record(&self, record: Arc<dyn RecordApi>) {
        self.inner.write().expect("media services lock").record = Some(record);
    }

    /// Return the current record provider, if any.
    ///
    /// 返回当前录制 provider（如有）。
    pub fn record(&self) -> Option<Arc<dyn RecordApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .record
            .clone()
    }

    /// Register the snapshot provider.
    ///
    /// 注册快照 provider。
    pub fn register_snapshot(&self, snapshot: Arc<dyn SnapshotApi>) {
        self.inner.write().expect("media services lock").snapshot = Some(snapshot);
    }

    /// Return the current snapshot provider, if any.
    ///
    /// 返回当前快照 provider（如有）。
    pub fn snapshot(&self) -> Option<Arc<dyn SnapshotApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .snapshot
            .clone()
    }

    /// Register the proxy provider.
    ///
    /// 注册代理 provider。
    pub fn register_proxy(&self, proxy: Arc<dyn cheetah_media_api::port::ProxyApi>) {
        self.inner.write().expect("media services lock").proxy = Some(proxy);
    }

    /// Return the current proxy provider, if any.
    ///
    /// 返回当前代理 provider（如有）。
    pub fn proxy(&self) -> Option<Arc<dyn cheetah_media_api::port::ProxyApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .proxy
            .clone()
    }

    /// Register the RTP provider.
    ///
    /// 注册 RTP provider。
    pub fn register_rtp(&self, rtp: Arc<dyn RtpApi>) {
        self.inner.write().expect("media services lock").rtp = Some(rtp);
    }

    /// Return the current RTP provider, if any.
    ///
    /// 返回当前 RTP provider（如有）。
    pub fn rtp(&self) -> Option<Arc<dyn RtpApi>> {
        self.inner.read().expect("media services lock").rtp.clone()
    }

    /// Register the WebRTC provider.
    ///
    /// 注册 WebRTC provider。
    pub fn register_webrtc(&self, webrtc: Arc<dyn WebRtcApi>) {
        self.inner.write().expect("media services lock").webrtc = Some(webrtc);
    }

    /// Return the current WebRTC provider, if any.
    ///
    /// 返回当前 WebRTC provider（如有）。
    pub fn webrtc(&self) -> Option<Arc<dyn WebRtcApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .webrtc
            .clone()
    }

    /// Register the server admin provider.
    ///
    /// 注册服务器管理 provider。
    pub fn register_server_admin(&self, server_admin: Arc<dyn ServerAdminApi>) {
        self.inner
            .write()
            .expect("media services lock")
            .server_admin = Some(server_admin);
    }

    /// Return the current server admin provider, if any.
    ///
    /// 返回当前服务器管理 provider（如有）。
    pub fn server_admin(&self) -> Option<Arc<dyn ServerAdminApi>> {
        self.inner
            .read()
            .expect("media services lock")
            .server_admin
            .clone()
    }
}

#[derive(Default)]
struct MediaProviderRegistry {
    control: Option<Arc<dyn MediaControlApi>>,
    publish_subscribe: Option<Arc<dyn PublishSubscribeApi>>,
    record: Option<Arc<dyn RecordApi>>,
    snapshot: Option<Arc<dyn SnapshotApi>>,
    proxy: Option<Arc<dyn cheetah_media_api::port::ProxyApi>>,
    rtp: Option<Arc<dyn RtpApi>>,
    webrtc: Option<Arc<dyn WebRtcApi>>,
    server_admin: Option<Arc<dyn ServerAdminApi>>,
}

impl MediaProviderRegistry {
    fn empty() -> Self {
        Self::default()
    }
}
