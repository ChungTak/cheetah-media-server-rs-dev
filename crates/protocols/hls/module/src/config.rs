use serde::{Deserialize, Serialize};

/// `HlsModuleConfig` data structure.
/// `HlsModuleConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HlsModuleConfig {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `listen` field of type `String`.
    /// `listen` 字段，类型为 `String`.
    pub listen: String,
    /// Target segment duration in milliseconds.
    pub segment_duration_ms: u64,
    /// Maximum number of segments kept in the ring buffer per stream.
    pub segment_count: usize,
    /// Number of segments required before the stream is considered ready.
    pub ready_threshold: usize,
    /// Force segment cut even without keyframe after this multiple of segment_duration_ms.
    pub force_segment_after_ms: u64,
    /// Player session timeout in seconds (evict inactive sessions).
    pub session_timeout_secs: u64,
    /// How long to keep a muxer alive after its publisher disconnects so late-joining
    /// clients can finish the stream (in seconds). When set to 0, the muxer is removed
    /// immediately on EOS — useful only for tests and pure passthrough setups.
    #[serde(default = "default_concluded_retention_secs")]
    pub concluded_retention_secs: u64,
    /// Enable on-demand HLS generation (stop muxing when no viewers).
    #[serde(default)]
    pub hls_demand: bool,
    /// Force first segments to cut immediately on keyframe for fast stream discovery.
    #[serde(default)]
    pub fast_register: bool,
    /// Container format: "ts" (default) or "fmp4" (reserved for future).
    #[serde(default = "default_container")]
    pub container: String,
    /// Enable Low-Latency HLS (requires fMP4 container).
    #[serde(default = "default_true")]
    pub ll_hls_enabled: bool,
    /// LL-HLS part target duration in milliseconds (default 200).
    #[serde(default = "default_part_target_ms")]
    pub part_target_ms: u64,
    /// LL-HLS packaging mode: "demuxed-av" (default), "video-only", "muxed".
    #[serde(default = "default_ll_hls_packaging_mode")]
    pub ll_hls_packaging_mode: String,
    /// Maximum pending blocking requests per stream (default 10).
    #[serde(default = "default_max_pending_requests")]
    pub max_pending_requests: usize,
    /// Blocking request timeout in milliseconds (default 30000).
    #[serde(default = "default_blocking_timeout_ms")]
    pub blocking_timeout_ms: u64,
    /// CDN Bearer token secret (empty = CDN mode disabled).
    #[serde(default)]
    pub cdn_secret: String,
    /// CDN Origin mode: skip per-connection session management.
    #[serde(default)]
    pub origin_mode: bool,
    /// Enable stream_key validation on segment/part requests.
    #[serde(default)]
    pub stream_key_validation: bool,
    /// Cache-Control configuration.
    #[serde(default)]
    pub cache_control: CacheControlConfig,
    /// Maximum concurrent sessions per stream (0 = unlimited).
    #[serde(default)]
    pub max_sessions_per_stream: usize,
    /// HLS recording mode configuration.
    #[serde(default)]
    pub recording: HlsRecordingConfig,
    /// HTTPS/TLS configuration (optional).
    #[serde(default)]
    pub tls: Option<HlsTlsConfig>,
    /// Master playlist multi-bitrate variants (optional).
    #[serde(default)]
    pub master_playlists: Vec<HlsMasterPlaylistConfig>,
    /// Enable writing HLS segments to disk.
    #[serde(default)]
    pub file_output: HlsFileOutputConfig,
    /// HLS pull jobs (relay from remote HLS sources).
    #[serde(default)]
    pub pull_jobs: Vec<HlsPullJobConfig>,
}

/// Fine-grained Cache-Control header configuration.
/// Values: -1 = don't set header, 0 = no-cache/no-store, >0 = max-age seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheControlConfig {
    /// `master_playlist_max_age` field of type `i32`.
    /// `master_playlist_max_age` 字段，类型为 `i32`.
    pub master_playlist_max_age: i32,
    /// `chunklist_max_age` field of type `i32`.
    /// `chunklist_max_age` 字段，类型为 `i32`.
    pub chunklist_max_age: i32,
    /// `chunklist_with_directives_max_age` field of type `i32`.
    /// `chunklist_with_directives_max_age` 字段，类型为 `i32`.
    pub chunklist_with_directives_max_age: i32,
    /// `segment_max_age` field of type `i32`.
    /// `segment_max_age` 字段，类型为 `i32`.
    pub segment_max_age: i32,
    /// `partial_segment_max_age` field of type `i32`.
    /// `partial_segment_max_age` 字段，类型为 `i32`.
    pub partial_segment_max_age: i32,
}

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HlsRecordingConfig {
    /// Enable HLS recording mode (keep all segments, generate VOD playlist).
    pub enabled: bool,
    /// Maximum recording duration in seconds (0 = unlimited).
    pub max_duration_secs: u64,
    /// Maximum number of segments to keep (0 = unlimited).
    pub max_segments: usize,
    /// Generate VOD playlist with EXT-X-ENDLIST on stream end.
    pub generate_vod_playlist: bool,
}

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

/// HTTPS/TLS configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HlsTlsConfig {
    /// `cert_path` field of type `String`.
    /// `cert_path` 字段，类型为 `String`.
    pub cert_path: String,
    /// `key_path` field of type `String`.
    /// `key_path` 字段，类型为 `String`.
    pub key_path: String,
}

/// Master playlist multi-bitrate configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HlsMasterPlaylistConfig {
    /// Virtual stream name for the master playlist URL.
    pub name: String,
    /// Variant streams.
    pub variants: Vec<HlsVariantConfig>,
}

/// A single variant in a master playlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HlsVariantConfig {
    /// Stream key of the source stream.
    pub stream_key: String,
    /// Bandwidth in bits/sec.
    pub bandwidth: u64,
    /// Resolution (e.g., "1920x1080").
    #[serde(default)]
    pub resolution: Option<String>,
}

/// Configuration for HLS file output (disk-based segments).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HlsFileOutputConfig {
    /// Enable writing segments to disk.
    pub enabled: bool,
    /// Root directory for HLS file output.
    pub output_dir: String,
    /// Storage mode: "memory" (default), "disk", or "hybrid".
    pub storage_mode: String,
    /// Maximum number of segment files to retain on disk per stream.
    pub max_disk_segments: usize,
    /// Number of extra segments to retain on disk after removal from m3u8.
    pub segment_retain: usize,
    /// Delay in seconds before deleting files after stream ends.
    pub delete_delay_secs: u64,
    /// Whether to clean up stream directory when stream ends.
    pub cleanup_on_unpublish: bool,
}

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HlsPullJobConfig {
    /// `name` field of type `String`.
    /// `name` 字段，类型为 `String`.
    pub name: String,
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// Remote HLS source URL (master or media playlist).
    pub source_url: String,
    /// Local stream key to publish pulled content as.
    pub target_stream_key: String,
    /// Retry backoff in milliseconds.
    pub retry_backoff_ms: u64,
    /// Maximum retry backoff in milliseconds.
    pub max_retry_backoff_ms: u64,
}

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

impl HlsModuleConfig {
    /// Creates `value` from input.
    /// 创建 `值` 来自 输入.
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// `default_json` function.
    /// `default_json` 函数.
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap()
    }
}
