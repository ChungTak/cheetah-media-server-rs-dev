/// SRT role, stream mode, payload kind, and session options.
///
/// SRT 角色、流模式、负载类型与会话选项。
pub mod config;
/// SRT core error types and result aliases.
///
/// SRT core 错误类型与结果别名。
pub mod error;
/// SRT core command, input, output, and event types.
///
/// SRT core 命令、输入、输出与事件类型。
pub mod session;
/// SRT stream id parsing (access-control and plain key).
///
/// SRT stream id 解析（访问控制与普通密钥）。
pub mod stream_id;
/// SRT URL parsing.
///
/// SRT URL 解析。
pub mod url;
/// SRT version encoding and comparison helpers.
///
/// SRT 版本编码与比较辅助。
pub mod version;

pub use config::{
    SrtEncryptionOptions, SrtKeyLength, SrtPayloadKind, SrtRole, SrtSessionOptions, SrtStreamMode,
};
pub use error::{SrtCoreError, SrtCoreResult};
pub use session::{
    SrtCoreCommand, SrtCoreEvent, SrtCoreInput, SrtCoreOutput, SrtSessionId, SrtStatsSnapshot,
};
pub use stream_id::{
    parse_srt_stream_id, parse_srt_stream_id_with_options, ParsedSrtStreamId, StreamIdParseOptions,
};
pub use url::{parse_srt_url, ParsedSrtUrl};
pub use version::{
    format_srt_version, parse_srt_version, version_at_least, SRT_VERSION_1_3_0, SRT_VERSION_1_5_0,
};
