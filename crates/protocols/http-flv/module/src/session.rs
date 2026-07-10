use cheetah_http_flv_driver_tokio::HttpFlvConnectionId;

/// `HttpFlvPlaySession` data structure.
/// `HttpFlvPlaySession` 数据结构.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct HttpFlvPlaySession {
    /// `connection_id` field of type `HttpFlvConnectionId`.
    /// `connection_id` 字段，类型为 `HttpFlvConnectionId`.
    pub connection_id: HttpFlvConnectionId,
}
