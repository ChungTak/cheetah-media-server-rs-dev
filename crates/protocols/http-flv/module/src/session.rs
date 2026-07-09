use cheetah_http_flv_driver_tokio::HttpFlvConnectionId;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct HttpFlvPlaySession {
    pub connection_id: HttpFlvConnectionId,
}
