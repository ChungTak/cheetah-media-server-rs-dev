use std::net::SocketAddr;

use cheetah_rtmp_core::RtmpUrl;
use cheetah_sdk::{BackpressurePolicy, ProcessingPolicy, SdkError, TrackSelection};
use serde::{Deserialize, Serialize};

use crate::route::parse_stream_key_spec;

/// RTMP module configuration, including listen endpoints, jobs, and thresholds.
///
/// RTMP 模块配置，包含监听端点、任务与告警阈值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtmpModuleConfig {
    /// 是否启用 RTMP module。
    pub enabled: bool,
    /// RTMP TCP 监听地址，格式为 `ip:port`。
    pub listen: String,
    /// RTMPS (TLS) 配置。
    pub tls: RtmpTlsModuleConfig,
    /// 单连接发送队列容量（单位：消息/包）。
    pub write_queue_capacity: usize,
    /// 播放端等待源流上线的超时（毫秒）。
    pub play_wait_source_timeout_ms: u64,
    /// 每个订阅者的缓存队列容量（单位：帧）。
    pub subscriber_queue_capacity: usize,
    /// 订阅者背压策略，用于慢消费者处理。
    pub subscriber_backpressure: BackpressurePolicy,
    /// 新订阅者启动阶段可回放的最大帧数。
    pub bootstrap_max_frames: usize,
    /// 是否在无音频阶段补静音 AAC（兼容部分播放器）。
    pub enable_add_mute: bool,
    /// 是否向播放端发送 onMetaData 等元数据消息。
    pub emit_play_metadata: bool,
    /// 可观测性告警阈值配置。
    pub alert_thresholds: RtmpAlertThresholds,
    /// 拉流任务列表（RTMP -> 本地流）。
    pub pull_jobs: Vec<RtmpPullJobConfig>,
    /// 推流任务列表（本地流 -> RTMP）。
    pub push_jobs: Vec<RtmpPushJobConfig>,
    /// 转发任务列表（远程 RTMP -> 本地 -> 远程 RTMP）。
    pub relay_jobs: Vec<RtmpRelayJobConfig>,
    /// 鉴权配置。
    pub auth: RtmpAuthConfig,
    /// 发布者断连后保活窗口（毫秒）。在此窗口内同一 StreamKey 重新 publish 可恢复，
    /// 订阅者无感知。0 = 禁用（默认）。
    pub publish_keepalive_ms: u64,
    /// Paced sender 最小发送间隔（毫秒）。启用后对每个播放连接限制发送速率，
    /// 避免突发流量导致客户端缓冲区溢出。0 = 禁用（默认）。
    pub paced_sender_ms: u64,
    /// 直接代理模式。启用后所有编码的原始 RTMP 包都附带 side data，
    /// RTMP→RTMP 转发时跳过 demux/remux，降低 CPU 开销。
    /// 注意：直接代理流不支持跨协议转发。
    pub direct_proxy: bool,
}

/// Configuration for an RTMP pull job (remote RTMP -> local stream).
///
/// RTMP 拉流任务配置（远程 RTMP -> 本地流）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtmpPullJobConfig {
    /// 任务名称，必须唯一且非空。
    pub name: String,
    /// 是否启用该拉流任务。
    pub enabled: bool,
    /// 源 RTMP URL，例如 `rtmp://host/app/stream`。
    pub source_url: String,
    /// 拉流写入到本地的目标流标识。
    pub target_stream_key: String,
    /// 选择要保留的轨道（All / AudioOnly / VideoOnly）。
    pub track_selection: TrackSelection,
    /// 处理策略：Passthrough、Auto 或 Transcode。
    pub processing_policy: ProcessingPolicy,
    /// 首次/基础重试退避时间（毫秒）。
    pub retry_backoff_ms: u64,
    /// 最大重试退避时间（毫秒）。
    pub max_retry_backoff_ms: u64,
}

/// Configuration for an RTMP push job (local stream -> remote RTMP).
///
/// RTMP 推流任务配置（本地流 -> 远程 RTMP）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtmpPushJobConfig {
    /// 任务名称，必须唯一且非空。
    pub name: String,
    /// 是否启用该推流任务。
    pub enabled: bool,
    /// 推流读取的本地源流标识。
    pub source_stream_key: String,
    /// 目标 RTMP URL，例如 `rtmp://host/app/stream`。
    pub target_url: String,
    /// 选择要保留的轨道（All / AudioOnly / VideoOnly）。
    pub track_selection: TrackSelection,
    /// 处理策略：Passthrough、Auto 或 Transcode。
    pub processing_policy: ProcessingPolicy,
    /// 首次/基础重试退避时间（毫秒）。
    pub retry_backoff_ms: u64,
    /// 最大重试退避时间（毫秒）。
    pub max_retry_backoff_ms: u64,
}

/// Configuration for an RTMP relay job (remote -> local -> remote).
///
/// RTMP 转发任务配置（远程 -> 本地 -> 远程）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtmpRelayJobConfig {
    /// 任务名称，必须唯一且非空。
    pub name: String,
    /// 是否启用该转发任务。
    pub enabled: bool,
    /// 源 RTMP URL（拉流端），例如 `rtmp://source/app/stream`。
    pub source_url: String,
    /// 目标 RTMP URL（推流端），例如 `rtmp://target/app/stream`。
    pub target_url: String,
    /// 本地中转流标识（可选，默认从 source_url 提取）。
    pub stream_key: String,
    /// 首次/基础重试退避时间（毫秒）。
    pub retry_backoff_ms: u64,
    /// 最大重试退避时间（毫秒）。
    pub max_retry_backoff_ms: u64,
}

/// Observability alert thresholds for the RTMP module.
///
/// RTMP 模块可观测性告警阈值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtmpAlertThresholds {
    /// 起播等待告警阈值（毫秒）。
    pub startup_timeout_ms: u64,
    /// 时间戳逆序修正计数告警阈值。
    pub timestamp_repair_count: u64,
    /// 队列回压丢帧计数告警阈值。
    pub queue_drop_count: u64,
}

/// RTMPS (TLS) listener configuration.
///
/// RTMPS (TLS) 监听配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtmpTlsModuleConfig {
    /// 是否启用 RTMPS 监听。
    pub enabled: bool,
    /// RTMPS 监听地址，格式为 `ip:port`。
    pub listen: String,
    /// TLS 证书文件路径（PEM 格式）。
    pub cert_path: String,
    /// TLS 私钥文件路径（PEM 格式）。
    pub key_path: String,
    /// TLS 握手超时（毫秒）。
    pub handshake_timeout_ms: u64,
}

impl Default for RtmpPullJobConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            source_url: String::new(),
            target_stream_key: String::new(),
            track_selection: TrackSelection::All,
            processing_policy: ProcessingPolicy::Passthrough,
            retry_backoff_ms: 500,
            max_retry_backoff_ms: 5_000,
        }
    }
}

impl Default for RtmpPushJobConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            source_stream_key: String::new(),
            target_url: String::new(),
            track_selection: TrackSelection::All,
            processing_policy: ProcessingPolicy::Passthrough,
            retry_backoff_ms: 500,
            max_retry_backoff_ms: 5_000,
        }
    }
}

impl Default for RtmpRelayJobConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            source_url: String::new(),
            target_url: String::new(),
            stream_key: String::new(),
            retry_backoff_ms: 1_000,
            max_retry_backoff_ms: 30_000,
        }
    }
}

impl Default for RtmpModuleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: "0.0.0.0:1935".to_string(),
            tls: RtmpTlsModuleConfig::default(),
            write_queue_capacity: 256,
            play_wait_source_timeout_ms: 15_000,
            subscriber_queue_capacity: 256,
            subscriber_backpressure: BackpressurePolicy::DropUntilNextKeyframe,
            bootstrap_max_frames: 150,
            enable_add_mute: false,
            emit_play_metadata: true,
            alert_thresholds: RtmpAlertThresholds::default(),
            pull_jobs: Vec::new(),
            push_jobs: Vec::new(),
            relay_jobs: Vec::new(),
            auth: RtmpAuthConfig::default(),
            publish_keepalive_ms: 0,
            paced_sender_ms: 0,
            direct_proxy: false,
        }
    }
}

impl Default for RtmpAlertThresholds {
    fn default() -> Self {
        Self {
            startup_timeout_ms: 3_000,
            timestamp_repair_count: 32,
            queue_drop_count: 64,
        }
    }
}

impl Default for RtmpTlsModuleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "0.0.0.0:1936".to_string(),
            cert_path: String::new(),
            key_path: String::new(),
            handshake_timeout_ms: 5_000,
        }
    }
}

/// RTMP publish/play authorization configuration.
///
/// RTMP 发布/播放鉴权配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtmpAuthConfig {
    /// 是否启用鉴权。
    pub enabled: bool,
    /// 发布鉴权令牌（URL query 参数 `token` 匹配即通过）。
    /// 为空时不校验发布请求。
    pub publish_token: String,
    /// 播放鉴权令牌。为空时不校验播放请求。
    pub play_token: String,
}

impl RtmpModuleConfig {
    /// Parses the module configuration from a JSON value and validates it.
    ///
    /// 从 JSON 值解析并校验模块配置。
    pub fn from_value(value: serde_json::Value) -> Result<Self, SdkError> {
        let cfg: Self = serde_json::from_value(value)
            .map_err(|err| SdkError::InvalidArgument(format!("invalid rtmp config: {err}")))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validates the RTMP module configuration, including listen addresses, job specs, and thresholds.
    ///
    /// 校验 RTMP 模块配置，包括监听地址、任务规格与告警阈值。
    pub fn validate(&self) -> Result<(), SdkError> {
        self.listen
            .parse::<SocketAddr>()
            .map_err(|err| SdkError::InvalidArgument(format!("invalid rtmp.listen: {err}")))?;
        if self.tls.enabled {
            self.tls.listen.parse::<SocketAddr>().map_err(|err| {
                SdkError::InvalidArgument(format!("invalid rtmp.tls.listen: {err}"))
            })?;
            if self.tls.cert_path.is_empty() {
                return Err(SdkError::InvalidArgument(
                    "rtmp.tls.cert_path must not be empty when tls is enabled".to_string(),
                ));
            }
            if self.tls.key_path.is_empty() {
                return Err(SdkError::InvalidArgument(
                    "rtmp.tls.key_path must not be empty when tls is enabled".to_string(),
                ));
            }
        }
        if self.write_queue_capacity == 0 {
            return Err(SdkError::InvalidArgument(
                "rtmp.write_queue_capacity must be > 0".to_string(),
            ));
        }
        if self.subscriber_queue_capacity == 0 {
            return Err(SdkError::InvalidArgument(
                "rtmp.subscriber_queue_capacity must be > 0".to_string(),
            ));
        }
        if self.bootstrap_max_frames == 0 {
            return Err(SdkError::InvalidArgument(
                "rtmp.bootstrap_max_frames must be > 0".to_string(),
            ));
        }
        if self.alert_thresholds.startup_timeout_ms == 0 {
            return Err(SdkError::InvalidArgument(
                "rtmp.alert_thresholds.startup_timeout_ms must be > 0".to_string(),
            ));
        }
        if self.alert_thresholds.timestamp_repair_count == 0 {
            return Err(SdkError::InvalidArgument(
                "rtmp.alert_thresholds.timestamp_repair_count must be > 0".to_string(),
            ));
        }
        if self.alert_thresholds.queue_drop_count == 0 {
            return Err(SdkError::InvalidArgument(
                "rtmp.alert_thresholds.queue_drop_count must be > 0".to_string(),
            ));
        }
        if self.play_wait_source_timeout_ms > 0
            && self.alert_thresholds.startup_timeout_ms > self.play_wait_source_timeout_ms
        {
            return Err(SdkError::InvalidArgument(
                "rtmp.alert_thresholds.startup_timeout_ms must be <= rtmp.play_wait_source_timeout_ms when timeout is enabled".to_string(),
            ));
        }
        for (idx, job) in self.pull_jobs.iter().enumerate() {
            if !job.enabled {
                continue;
            }
            if job.name.trim().is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.pull_jobs[{idx}].name must not be empty"
                )));
            }
            if RtmpUrl::parse(job.source_url.trim()).is_err() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.pull_jobs[{idx}].source_url is invalid"
                )));
            }
            if parse_stream_key_spec(job.target_stream_key.trim()).is_none() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.pull_jobs[{idx}].target_stream_key is invalid"
                )));
            }
            if job.retry_backoff_ms == 0 || job.max_retry_backoff_ms == 0 {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.pull_jobs[{idx}] backoff must be > 0"
                )));
            }
        }
        for (idx, job) in self.push_jobs.iter().enumerate() {
            if !job.enabled {
                continue;
            }
            if job.name.trim().is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.push_jobs[{idx}].name must not be empty"
                )));
            }
            if parse_stream_key_spec(job.source_stream_key.trim()).is_none() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.push_jobs[{idx}].source_stream_key is invalid"
                )));
            }
            if RtmpUrl::parse(job.target_url.trim()).is_err() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.push_jobs[{idx}].target_url is invalid"
                )));
            }
            if job.retry_backoff_ms == 0 || job.max_retry_backoff_ms == 0 {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.push_jobs[{idx}] backoff must be > 0"
                )));
            }
        }
        for (idx, job) in self.relay_jobs.iter().enumerate() {
            if !job.enabled {
                continue;
            }
            if job.name.trim().is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.relay_jobs[{idx}].name must not be empty"
                )));
            }
            if RtmpUrl::parse(job.source_url.trim()).is_err() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.relay_jobs[{idx}].source_url is invalid"
                )));
            }
            if RtmpUrl::parse(job.target_url.trim()).is_err() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.relay_jobs[{idx}].target_url is invalid"
                )));
            }
            if !job.stream_key.is_empty() && parse_stream_key_spec(job.stream_key.trim()).is_none()
            {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.relay_jobs[{idx}].stream_key is invalid"
                )));
            }
            if job.retry_backoff_ms == 0 || job.max_retry_backoff_ms == 0 {
                return Err(SdkError::InvalidArgument(format!(
                    "rtmp.relay_jobs[{idx}] backoff must be > 0"
                )));
            }
        }
        Ok(())
    }

    /// Returns the default configuration as a JSON value for schema registration.
    ///
    /// 返回默认配置的 JSON 值，用于 schema 注册。
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).expect("serialize default rtmp config")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_sdk::ProcessingPreset;

    #[test]
    fn default_has_alert_thresholds() {
        let cfg = RtmpModuleConfig::default();
        assert_eq!(cfg.alert_thresholds.startup_timeout_ms, 3_000);
        assert_eq!(cfg.alert_thresholds.timestamp_repair_count, 32);
        assert_eq!(cfg.alert_thresholds.queue_drop_count, 64);
        cfg.validate().expect("default config should validate");
    }

    #[test]
    fn reject_zero_alert_thresholds() {
        let cfg = RtmpModuleConfig {
            alert_thresholds: RtmpAlertThresholds {
                startup_timeout_ms: 0,
                timestamp_repair_count: 1,
                queue_drop_count: 1,
            },
            ..RtmpModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject zero startup threshold");
        assert!(err
            .to_string()
            .contains("rtmp.alert_thresholds.startup_timeout_ms"));
    }

    #[test]
    fn reject_startup_threshold_larger_than_play_wait_timeout() {
        let cfg = RtmpModuleConfig {
            play_wait_source_timeout_ms: 1_000,
            alert_thresholds: RtmpAlertThresholds {
                startup_timeout_ms: 1_001,
                timestamp_repair_count: 32,
                queue_drop_count: 64,
            },
            ..RtmpModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject unreachable startup timeout threshold");
        assert!(err
            .to_string()
            .contains("rtmp.alert_thresholds.startup_timeout_ms"));
    }

    #[test]
    fn job_config_serializes_track_selection_and_processing_policy() {
        let push = RtmpPushJobConfig {
            name: "test".to_string(),
            track_selection: TrackSelection::AudioOnly,
            processing_policy: ProcessingPolicy::Auto {
                preset: ProcessingPreset::Balanced,
            },
            ..RtmpPushJobConfig::default()
        };
        let json = serde_json::to_string(&push).unwrap();
        assert!(json.contains("\"track_selection\""));
        assert!(json.contains("\"processing_policy\""));
        let de: RtmpPushJobConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(de.track_selection, TrackSelection::AudioOnly);
        assert_eq!(de.processing_policy, push.processing_policy);
    }
}
