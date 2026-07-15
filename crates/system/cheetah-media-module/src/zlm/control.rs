//! ZLMediaKit-compatible media control endpoint handlers.
//!
//! ZLMediaKit 兼容的媒体控制端点处理函数。

use cheetah_media_api::command::{SessionQuery, StartRecordRequest};
use cheetah_media_api::ids::{FileHandle, SessionId};
use cheetah_media_api::media_file_store::FileRange;
use cheetah_media_api::model::{CloseReason, SessionKind};
use cheetah_media_api::port::MediaRequestContext;
use cheetah_sdk::{HttpHeader, HttpRequest, HttpResponse};

use crate::error::AdapterError;
use crate::zlm::{
    page_from_params, page_size_from_params, zlm_record_format, zlm_response, CloseStreamsResult,
    Data, Empty, KickSessionsResult, SessionItem, ZlmMediaHttpService, ZlmResponse, ZlmResult,
};

impl ZlmMediaHttpService {
    pub(crate) async fn get_media_player_list(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let mut query = SessionQuery {
            vhost: params["vhost"].as_str().map(String::from),
            app: params["app"].as_str().map(String::from),
            stream: params["stream"].as_str().map(String::from),
            kind: Some(SessionKind::Player),
            page: page_from_params(&params),
            page_size: page_size_from_params(&params),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = self.control()?.list_sessions(ctx, query).await?;
        let items: Vec<SessionItem> = page.items.into_iter().map(SessionItem::from).collect();
        Ok(zlm_response(ZlmResponse::ok(Data::new(items))))
    }

    pub(crate) async fn close_streams(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let control = self.control()?;
        let mut query = SessionQuery {
            vhost: Some(key.vhost.0.clone()),
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            page_size: SessionQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        query.clamp_page_size();
        let page = control.list_sessions(ctx, query).await?;
        let count_hit = page.items.len() as u64;
        let count_closed = if control
            .kick_stream(ctx, &key, CloseReason::Kicked)
            .await
            .is_ok()
        {
            count_hit
        } else {
            0
        };
        Ok(zlm_response(ZlmResponse::ok(CloseStreamsResult {
            count_hit,
            count_closed,
        })))
    }

    pub(crate) async fn kick_sessions(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let control = self.control()?;

        if let Some(id) = params["session_id"]
            .as_str()
            .or_else(|| params["id"].as_str())
        {
            control
                .kick_session(ctx, &SessionId(id.to_string()), CloseReason::Kicked)
                .await?;
            return Ok(zlm_response(ZlmResponse::with_msg(0, "success", Empty)));
        }

        let key = self.parse_media_key(&params)?;
        let mut query = SessionQuery {
            vhost: Some(key.vhost.0.clone()),
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            page_size: SessionQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        query.clamp_page_size();
        let page = control.list_sessions(ctx, query).await?;
        let count_hit = page.items.len() as u64;
        for session in &page.items {
            let _ = control
                .kick_session(ctx, &session.session_id, CloseReason::Kicked)
                .await;
        }
        Ok(zlm_response(ZlmResponse::with_msg(
            0,
            "success",
            KickSessionsResult { count_hit },
        )))
    }

    pub(crate) async fn start_record_task(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = zlm_record_format(&params["type"])?;
        let mut ctx = ctx.clone();
        ctx.idempotency_key = params["id"].as_str().map(|s| s.to_string());

        let request = StartRecordRequest {
            media_key: key,
            format: format.clone(),
            template: cheetah_media_api::model::RecordTemplate::Continuous,
            segment_duration_ms: None,
            max_segments: None,
            storage_policy: cheetah_media_api::model::StoragePolicy::default(),
            idempotency_key: ctx
                .idempotency_key
                .clone()
                .map(cheetah_media_api::ids::IdempotencyKey),
        };
        let _task = record_api.start_record(&ctx, request).await?;
        Ok(zlm_response(ZlmResponse::ok(ZlmResult { result: true })))
    }

    pub(crate) async fn download_file(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let file_path = params["file_path"]
            .as_str()
            .or_else(|| params["fileId"].as_str())
            .ok_or_else(|| AdapterError::InvalidRequest("file_path is required".to_string()))?;
        let handle = FileHandle(file_path.to_string());
        let range = parse_range_header(&req.headers);
        let is_range = range.is_some();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let download = self
            .ctx
            .media_file_store
            .resolve_download(ctx, &handle, range, None, now)
            .map_err(AdapterError::Media)?;

        let mut headers = vec![
            HttpHeader {
                name: "content-type".to_string(),
                value: download.content_type.clone(),
            },
            HttpHeader {
                name: "content-length".to_string(),
                value: download.body.len().to_string(),
            },
        ];
        if let Some(range) = download.range {
            if download.body.is_empty() {
                return Ok(HttpResponse {
                    status: 416,
                    headers: vec![HttpHeader {
                        name: "content-range".to_string(),
                        value: format!("bytes */{}", download.total_size),
                    }],
                    body: bytes::Bytes::new(),
                });
            }
            let last = range.start + download.body.len() as u64 - 1;
            headers.push(HttpHeader {
                name: "content-range".to_string(),
                value: format!("bytes {}-{}/{}", range.start, last, download.total_size),
            });
        }
        Ok(HttpResponse {
            status: if is_range { 206 } else { 200 },
            headers,
            body: download.body,
        })
    }
}

fn parse_range_header(headers: &[HttpHeader]) -> Option<FileRange> {
    let value = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("range"))?
        .value
        .as_str();
    let value = value.strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    let start: u64 = start.parse().ok()?;
    let end: u64 = end.parse().ok()?;
    Some(FileRange {
        start,
        end: Some(end),
        is_suffix: false,
    })
}

#[cfg(test)]
mod tests {
    use cheetah_sdk::HttpHeader;

    use super::parse_range_header;

    #[test]
    fn parse_range_header_uses_inclusive_end() {
        let headers = vec![HttpHeader {
            name: "Range".to_string(),
            value: "bytes=0-9".to_string(),
        }];
        let range = parse_range_header(&headers).expect("valid range");
        assert_eq!(range.start, 0);
        assert_eq!(range.end, Some(9));
        assert!(!range.is_suffix);
    }
}
