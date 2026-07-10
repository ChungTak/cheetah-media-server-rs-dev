//! HLS module configuration structures.
//!
//! Defines the serde-decodable config tree, defaults, and helper methods used by
//! the engine to initialize and validate the HLS module.
//!
//! HLS 模块配置结构。
//!
//! 定义引擎用于初始化与校验 HLS 模块的可 serde 解码配置树、默认值和辅助方法。
//!

use serde::{Deserialize, Serialize};

/// Top-level HLS module configuration.
///
/// Drives the HTTP server binding, segment/part timing, LL-HLS packaging, CDN
/// origin mode, session limits, and disk/recording options.
///
/// HLS 模块顶层配置。
///
/// 控制 HTTP 服务绑定、分段/分片时序、LL-HLS 封装、CDN 源站模式、
/// 会话限制以及磁盘/录制选项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HlsModuleConfig {
    pub enabled: bool,
    pub listen: String,
    pub segment_duration_ms: u64,
    pub segment_count: usize,
    pub ready_threshold: usize,
    pub force_segment_after_ms: u64,
    pub session_timeout_secs: u64,
    #[serde(default = "default_concluded_retention_secs")]
    pub concluded_retention_secs: u64,
    #[serde(default)]
    pub hls_demand: bool,
    #[serde(default)]
    pub fast_register: bool,
    #[serde(default = "default_container")]
    pub container: String,
    #[serde(default = "default_true")]
    pub ll_hls_enabled: bool,
    #[serde(default = "default_part_target_ms")]
    pub part_target_ms: u64,
    #[serde(default = "default_ll_hls_packaging_mode")]
    pub ll_hls_packaging_mode: String,
    #[serde(default = "default_max_pending_requests")]
    pub max_pending_requests: usize,
    #[serde(default = "default_blocking_timeout_ms")]
    pub blocking_timeout_ms: u64,
    #[serde(default)]
    pub cdn_secret: String,
    #[serde(default)]
    pub origin_mode: bool,
    #[serde(default)]
    pub stream_key_validation: bool,
    #[serde(default)]
    pub cache_control: CacheControlConfig,
    #[serde(default)]
    pub max_sessions_per_stream: usize,
    #[serde(default)]
    pub recording: HlsRecordingConfig,
    #[serde(default)]
    pub tls: Option<HlsTlsConfig>,
    #[serde(default)]
    pub master_playlists: Vec<HlsMasterPlaylistConfig>,
    #[serde(default)]
    pub file_output: HlsFileOutputConfig,
    #[serde(default)]
    pub pull_jobs: Vec<HlsPullJobConfig>,
}

/// Fine-grained Cache-Control header configuration.
///
/// Values map to HTTP `max-age`, `no-cache/no-store`, or omit the header.
///
/// 细粒度的 Cache-Control 头部配置。
///
/// 取值映射到 HTTP `max-age`、`no-cache/no-store` 或省略头部。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheControlConfig {
    pub master_playlist_max_age: i32,
    pub chunklist_max_age: i32,
    pub chunklist_with_directives_max_age: i32,
    pub segment_max_age: i32,
    pub partial_segment_max_age: i32,
}

/// Default Cache-Control: live chunklists uncached, with-directives cached for 60 s.
///
/// 默认 Cache-Control：直播分片列表不缓存，带指令分片列表缓存 60 秒。
impl Default for CacheControlConfig {
    fn default() -> Self {
        Self {
            master_playlist_max_age: 0,
            chunklist_max_age: 0,
            chunklist_with_directives_max_age: 60,
            segment_max_age: -1,
            partial_segment_max_age: -1,
        }
    }
}

/// Configuration for HLS recording mode.
///
/// When enabled, the muxer keeps a longer segment window and can emit an
/// `EXT-X-ENDLIST` VOD playlist on stream end.
///
/// HLS 录制模式配置。
///
/// 启用时，复用器保留更长的分段窗口，并可在流结束时生成带 `EXT-X-ENDLIST` 的 VOD 播放列表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HlsRecordingConfig {
    pub enabled: bool,
    pub max_duration_secs: u64,
    pub max_segments: usize,
    pub generate_vod_playlist: bool,
}

/// Default recording config: disabled with unlimited duration and VOD playlist.
///
/// 默认录制配置：禁用、时长无限制并生成 VOD 播放列表。
impl Default for HlsRecordingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_duration_secs: 0,
            max_segments: 0,
            generate_vod_playlist: true,
        }
    }
}

/// HTTPS/TLS configuration for the HLS HTTP server.
///
/// HLS HTTP 服务器的 HTTPS/TLS 配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HlsTlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

/// Multi-bitrate master playlist configuration.
///
/// 多码率主播放列表配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HlsMasterPlaylistConfig {
    pub name: String,
    pub variants: Vec<HlsVariantConfig>,
}

/// A single variant in a master playlist.
///
/// 主播放列表中的单个码率变体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HlsVariantConfig {
    pub stream_key: String,
    pub bandwidth: u64,
    #[serde(default)]
    pub resolution: Option<String>,
}

/// Configuration for writing HLS segments to disk.
///
/// Controls memory/disk/hybrid storage and the cleanup policy after a stream ends.
///
/// HLS 分段写入磁盘的配置。
///
/// 控制内存/磁盘/混合存储以及流结束后的清理策略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HlsFileOutputConfig {
    pub enabled: bool,
    pub output_dir: String,
    pub storage_mode: String,
    pub max_disk_segments: usize,
    pub segment_retain: usize,
    pub delete_delay_secs: u64,
    pub cleanup_on_unpublish: bool,
}

/// Default file output: disabled, writing to `/tmp/hls` with cleanup.
///
/// 默认文件输出：禁用，写入 `/tmp/hls` 并启用清理。
impl Default for HlsFileOutputConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            output_dir: "/tmp/hls".to_string(),
            storage_mode: "memory".to_string(),
            max_disk_segments: 20,
            segment_retain: 2,
            delete_delay_secs: 10,
            cleanup_on_unpublish: true,
        }
    }
}

/// Configuration for a single HLS pull job.
///
/// Pull jobs relay remote HLS sources into the engine as local streams.
///
/// 单个 HLS 拉流任务的配置。
///
/// 拉流任务将远程 HLS 源作为本地流中继到引擎。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HlsPullJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_stream_key: String,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}

/// Default pull job: disabled, with 1 s initial backoff and 10 s max backoff.
///
/// 默认拉流任务：禁用，初始退避 1 秒，最大退避 10 秒。
impl Default for HlsPullJobConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: false,
            source_url: String::new(),
            target_stream_key: String::new(),
            retry_backoff_ms: 1000,
            max_retry_backoff_ms: 10000,
        }
    }
}

fn default_container() -> String {
    "ts".to_string()
}

fn default_true() -> bool {
    true
}

fn default_part_target_ms() -> u64 {
    200
}

fn default_ll_hls_packaging_mode() -> String {
    "video-only".to_string()
}

fn default_max_pending_requests() -> usize {
    10
}

fn default_concluded_retention_secs() -> u64 {
    30
}

fn default_blocking_timeout_ms() -> u64 {
    30000
}

/// Default HLS module config: 8080-bound TS HLS with 4 s segments and LL-HLS.
///
/// 默认 HLS 模块配置：绑定 8080 端口的 TS HLS，4 秒分段并启用 LL-HLS。
impl Default for HlsModuleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: "0.0.0.0:8088".to_string(),
            segment_duration_ms: 4000,
            segment_count: 5,
            ready_threshold: 1,
            force_segment_after_ms: 12000,
            session_timeout_secs: 10,
            concluded_retention_secs: 30,
            hls_demand: false,
            fast_register: true,
            container: "ts".to_string(),
            ll_hls_enabled: true,
            part_target_ms: 200,
            ll_hls_packaging_mode: "demuxed-av".to_string(),
            max_pending_requests: 10,
            blocking_timeout_ms: 30000,
            cdn_secret: String::new(),
            origin_mode: false,
            stream_key_validation: false,
            cache_control: CacheControlConfig::default(),
            max_sessions_per_stream: 0,
            recording: HlsRecordingConfig::default(),
            tls: None,
            master_playlists: Vec::new(),
            file_output: HlsFileOutputConfig::default(),
            pull_jobs: Vec::new(),
        }
    }
}

/// Parse a JSON value into `HlsModuleConfig`.
///
/// Used by the engine config validation path to turn a module-specific config blob
/// into a typed struct.
///
/// 将 JSON 值解析为 `HlsModuleConfig`。
///
/// 引擎配置校验路径使用它将模块专属配置块转换为类型化结构体。
impl HlsModuleConfig {
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// Return the default config as a JSON value.
    ///
    /// Provides the schema default displayed in control plane and config editors.
    ///
    /// 以 JSON 值形式返回默认配置。
    ///
    /// 为控制面和配置编辑器提供默认 schema。
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap()
    }
}
