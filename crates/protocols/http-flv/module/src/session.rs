use cheetah_http_flv_driver_tokio::HttpFlvConnectionId;

/// `HttpFlvPlaySession` data structure.
/// `HttpFlvPlaySession` 数据结构。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct HttpFlvPlaySession {
    pub connection_id: HttpFlvConnectionId,
}
