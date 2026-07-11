use cheetah_http_flv_driver_tokio::HttpFlvConnectionId;

/// Placeholder for an HTTP-FLV play session.
///
/// Today the session state is tracked entirely by the server loop; this struct
/// is reserved for future per-connection metadata.
///
/// HTTP-FLV 播放会话占位符。
///
/// 目前会话状态完全由服务器循环维护；此结构为将来每个连接的元数据保留。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct HttpFlvPlaySession {
    pub connection_id: HttpFlvConnectionId,
}
