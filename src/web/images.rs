use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::header::{CACHE_CONTROL, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use axum::http::response::Builder;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use httpdate::{fmt_http_date, parse_http_date};

use crate::constants::IMAGE_CACHE_CONTROL;
use crate::error::HttpetError;

/// Cache headers derived from image metadata.
#[derive(Clone, Debug)]
pub(crate) struct ImageCacheHeaders {
    etag: Option<HeaderValue>,
    last_modified: Option<HeaderValue>,
    modified_at: Option<SystemTime>,
}

impl ImageCacheHeaders {
    /// Builds cache headers from filesystem metadata.
    pub(crate) fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        let modified_at = metadata.modified().ok();
        let etag = build_etag(metadata.len(), modified_at);
        let last_modified =
            modified_at.and_then(|modified| HeaderValue::from_str(&fmt_http_date(modified)).ok());
        Self {
            etag,
            last_modified,
            modified_at,
        }
    }

    /// Returns the ETag header value, if available.
    pub(crate) fn etag(&self) -> Option<&HeaderValue> {
        self.etag.as_ref()
    }

    /// Returns the Last-Modified header value, if available.
    pub(crate) fn last_modified(&self) -> Option<&HeaderValue> {
        self.last_modified.as_ref()
    }
}

/// Applies image cache headers to a response builder.
pub(crate) fn apply_cache_headers(mut builder: Builder, cache: &ImageCacheHeaders) -> Builder {
    builder = builder.header(CACHE_CONTROL, IMAGE_CACHE_CONTROL.as_str());
    if let Some(etag) = cache.etag() {
        builder = builder.header(ETAG, etag.clone());
    }
    if let Some(last_modified) = cache.last_modified() {
        builder = builder.header(LAST_MODIFIED, last_modified.clone());
    }
    builder
}

/// Returns true when the request matches a not-modified response.
pub(crate) fn is_not_modified(headers: &HeaderMap, cache: &ImageCacheHeaders) -> bool {
    if let Some(if_none_match) = headers.get(IF_NONE_MATCH) {
        if let Ok(value) = if_none_match.to_str() {
            let value = value.trim();
            if value == "*" {
                return true;
            }
            if let Some(etag) = cache.etag().and_then(|value| value.to_str().ok())
                && value.split(',').any(|candidate| candidate.trim() == etag)
            {
                return true;
            }
        }
        return false;
    }

    if let (Some(if_modified_since), Some(modified_at)) =
        (headers.get(IF_MODIFIED_SINCE), cache.modified_at)
        && let Ok(value) = if_modified_since.to_str()
        && let Ok(since) = parse_http_date(value)
        && modified_at <= since
    {
        return true;
    }

    false
}

/// Builds a 304 response that preserves cache headers.
pub(crate) fn not_modified_response(cache: &ImageCacheHeaders) -> Result<Response, HttpetError> {
    let builder = Response::builder().status(StatusCode::NOT_MODIFIED);
    let builder = apply_cache_headers(builder, cache);
    builder.body(Body::empty()).map_err(HttpetError::from)
}

fn build_etag(size: u64, modified_at: Option<SystemTime>) -> Option<HeaderValue> {
    let suffix = match modified_at {
        Some(modified) => modified
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string()),
        None => "0".to_string(),
    };
    let value = format!("W/\"{}-{}\"", size, suffix);
    HeaderValue::from_str(&value).ok()
}
