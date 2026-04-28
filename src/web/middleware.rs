use axum::body::Body;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::Request;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE, HOST, TRANSFER_ENCODING};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{Redirect, Response};
use chrono::{SecondsFormat, Utc};
use reqwest::header::FORWARDED;
use serde::Serialize;

use std::net::{IpAddr, SocketAddr};

use super::prelude::*;
use super::{AppState, normalize_pet_name, views};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AnimalDomain {
    pub(crate) animal: Option<String>,
}

impl AnimalDomain {
    fn from_host(base_domain: &str, host: &str) -> Self {
        let host = host
            .split(':')
            .next()
            .unwrap_or(host)
            .trim_end_matches('.')
            .to_ascii_lowercase();
        let animal = animal_from_host(base_domain, &host);

        Self { animal }
    }
}

fn animal_from_host(base_domain: &str, host: &str) -> Option<String> {
    let www_domain = format!("www.{}", base_domain);

    if host == base_domain || host == www_domain {
        return None;
    }

    let suffix = format!(".{}", base_domain);
    let subdomain = host.strip_suffix(&suffix)?;
    let label = subdomain.split('.').next()?;
    if label.is_empty() {
        return None;
    }

    Some(normalize_pet_name(label))
}

impl FromRequestParts<AppState> for AnimalDomain {
    type Rejection = (StatusCode, &'static str);

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let host = parts
            .headers
            .get(HOST)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        async move {
            let host = host.ok_or((StatusCode::BAD_REQUEST, "Missing Host header"))?;
            Ok(Self::from_host(&state.base_domain, &host))
        }
    }
}

pub(crate) async fn admin_base_domain_only(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let host = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let host = normalize_host(host);

    if host == state.base_domain {
        return next.run(request).await;
    }

    let uri = request.uri().to_string();
    let target = format!("{}{}", state.base_url(), uri);
    Redirect::to(&target).into_response()
}

pub(crate) async fn not_found_template(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let response = next.run(request).await;
    if response.status() != StatusCode::NOT_FOUND {
        return response;
    }

    let (parts, _body) = response.into_parts();
    let mut not_found = views::not_found_response(&state).await;
    let headers = not_found.headers_mut();
    for (name, value) in parts.headers.iter() {
        if name == CONTENT_TYPE || name == CONTENT_LENGTH || name == TRANSFER_ENCODING {
            continue;
        }
        headers.append(name, value.clone());
    }

    not_found
}

pub(crate) async fn request_logger(mut request: Request<Body>, next: Next) -> Response {
    let method = request.method().to_string();
    let uri = request.uri().to_string();
    let client_ip = client_ip_from_request(&request);
    let timestamp = current_timestamp();

    let x_forwarded_for = if let Some(header) = request.headers_mut().remove(X_FORWARDED_FOR) {
        parse_forwarded_for_header(&header, &client_ip)
    } else {
        None
    };
    let x_real_ip = if let Some(header) = request.headers_mut().remove(X_REAL_IP) {
        parse_real_ip_header(&header, &client_ip)
    } else {
        None
    };

    let forwarded = if let Some(header) = request.headers_mut().remove(FORWARDED) {
        parse_forwarded_header(&header)
            .inspect_err(|_| {
                warn_invalid_ip_header(
                    FORWARDED.as_str(),
                    &header_value_for_log(&header),
                    &client_ip,
                )
            })
            .ok()
            .flatten()
    } else {
        None
    };

    let response = next.run(request).await;
    let status = response.status().as_u16();
    RequestLog::new(
        &timestamp,
        &client_ip,
        &method,
        &uri,
        status,
        x_forwarded_for,
        x_real_ip,
        forwarded,
    )
    .print();
    response
}

fn normalize_host(host: &str) -> String {
    host.split(':')
        .next()
        .unwrap_or(host)
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn client_ip_from_request(request: &Request<Body>) -> String {
    if let Some(connect_info) = request.extensions().get::<ConnectInfo<SocketAddr>>() {
        return connect_info.0.ip().to_string();
    }
    "unknown".to_string()
}

fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[derive(Debug, Serialize)]
struct RequestLog<'a> {
    time: &'a str,
    src: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    forwarded_for: Option<Vec<IpAddr>>,
    http_method: &'a str,
    uri: &'a str,
    status: u16,

    #[serde(skip_serializing_if = "Option::is_none")]
    real_ip: Option<IpAddr>,

    #[serde(skip_serializing_if = "Option::is_none")]
    forwarded: Option<IpAddr>,
}

impl<'a> RequestLog<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        time: &'a str,
        src: &'a str,
        http_method: &'a str,
        uri: &'a str,
        status: u16,
        forwarded_for: Option<Vec<IpAddr>>,
        real_ip: Option<IpAddr>,
        forwarded: Option<IpAddr>,
    ) -> Self {
        Self {
            time,
            src,
            http_method,
            uri,
            status,
            forwarded_for,
            real_ip,
            forwarded,
        }
    }

    pub(crate) fn print(&self) {
        tracing::log::info!("{}", serde_json::json!(&self));
    }
}

const X_FORWARDED_FOR: &str = "x-forwarded-for";
const X_REAL_IP: &str = "x-real-ip";

fn parse_forwarded_for_header(header: &HeaderValue, client_ip: &str) -> Option<Vec<IpAddr>> {
    let value = header_value_for_log(header);
    let Ok(parsed) = header.to_str() else {
        warn_invalid_ip_header(X_FORWARDED_FOR, &value, client_ip);
        return None;
    };
    if value.trim().is_empty() {
        warn_invalid_ip_header(X_FORWARDED_FOR, &value, client_ip);
        return None;
    }
    let mut ips = Vec::new();
    for part in parsed.split(',') {
        let ip_str = part.trim();
        if ip_str.is_empty() {
            warn_invalid_ip_header(X_FORWARDED_FOR, &value, client_ip);
            return None;
        }
        let Ok(ip) = ip_str.parse() else {
            warn_invalid_ip_header(X_FORWARDED_FOR, &value, client_ip);
            return None;
        };
        ips.push(ip);
    }
    Some(ips)
}

fn parse_real_ip_header(header: &HeaderValue, client_ip: &str) -> Option<IpAddr> {
    let value = header_value_for_log(header);
    let Ok(parsed) = header.to_str() else {
        warn_invalid_ip_header(X_REAL_IP, &value, client_ip);
        return None;
    };
    let ip_str = parsed.trim();
    if ip_str.is_empty() {
        warn_invalid_ip_header(X_REAL_IP, &value, client_ip);
        return None;
    }
    let Ok(ip) = ip_str.parse() else {
        warn_invalid_ip_header(X_REAL_IP, &value, client_ip);
        return None;
    };
    Some(ip)
}

fn parse_forwarded_header(headervalue: &HeaderValue) -> Result<Option<IpAddr>, String> {
    let ip_str = headervalue
        .to_str()
        .map_err(|err| format!("Invalid header value: {}", err))?
        .trim();
    if ip_str.is_empty() {
        return Err("Empty header value".to_string());
    }
    let parts: Vec<&str> = ip_str.split(';').collect();
    for part in parts {
        if let Some(stripped) = part.trim().strip_prefix("for=") {
            let ip_str = stripped.trim_matches('"');
            return ip_str
                .parse()
                .map_err(|_| "Invalid IP address in Forwarded header".to_string())
                .map(Some);
        }
    }
    Ok(None)
}

fn header_value_for_log(header: &axum::http::HeaderValue) -> String {
    String::from_utf8_lossy(header.as_bytes()).to_string()
}

fn warn_invalid_ip_header(header: &str, value: &str, client_ip: &str) {
    tracing::warn!(
        client_ip = %client_ip,
        header = %header,
        value = %value,
        "Ignoring invalid forwarded IP header"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::http::Request;
    use axum::http::StatusCode;
    use axum::routing::get;
    use chrono::DateTime;
    use std::net::{IpAddr, Ipv4Addr};
    use tower::ServiceExt;

    #[test]
    fn current_timestamp_is_rfc3339_utc() {
        let timestamp = current_timestamp();
        let parsed = DateTime::parse_from_rfc3339(&timestamp).expect("parse timestamp");
        assert_eq!(parsed.offset().local_minus_utc(), 0);
        assert!(timestamp.ends_with('Z'));
    }

    #[test]
    fn new_handles_status_variants() {
        let cases = [200u16, 400u16, 404u16, 500u16];
        for status in cases {
            let log = RequestLog::new(
                "2026-02-03T12:34:56.789Z",
                "203.0.113.42",
                "POST",
                "/vote",
                status,
                None,
                None,
                None,
            );
            assert_eq!(log.status, status);
        }
    }

    #[test]
    fn client_ip_ignores_forwarded_headers() {
        let mut request = Request::builder()
            .uri("/")
            .header("x-forwarded-for", "203.0.113.1, 10.0.0.1")
            .header("x-real-ip", "198.51.100.2")
            .body(Body::empty())
            .expect("request");
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 5)),
            1234,
        )));

        assert_eq!(client_ip_from_request(&request), "192.0.2.5");
    }

    #[test]
    fn client_ip_falls_back_to_connect_info() {
        let mut request = Request::builder()
            .uri("/")
            .body(Body::empty())
            .expect("request");
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 5)),
            1234,
        )));

        assert_eq!(client_ip_from_request(&request), "192.0.2.5");
    }

    #[test]
    fn client_ip_returns_unknown_when_unavailable() {
        let request = Request::builder()
            .uri("/")
            .body(Body::empty())
            .expect("request");

        assert_eq!(client_ip_from_request(&request), "unknown");
    }

    #[test]
    fn parse_forwarded_for_header_accepts_valid_ips() {
        let request = Request::builder()
            .uri("/")
            .header("x-forwarded-for", "203.0.113.1, 198.51.100.2")
            .body(Body::empty())
            .expect("request");

        let parsed = parse_forwarded_for_header(
            request.headers().get(X_FORWARDED_FOR).expect("no header"),
            "192.0.2.5",
        )
        .expect("present");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)));
        assert_eq!(parsed[1], IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2)));
    }

    #[test]
    fn parse_forwarded_for_header_ignores_invalid_ips() {
        let request = Request::builder()
            .uri("/")
            .header("x-forwarded-for", "203.0.113.1, nope")
            .body(Body::empty())
            .expect("request");

        assert!(
            parse_forwarded_for_header(
                request.headers().get(X_FORWARDED_FOR).expect("no header"),
                "192.0.2.5"
            )
            .is_none()
        );
    }

    #[test]
    fn parse_real_ip_header_accepts_valid_ip() {
        let request = Request::builder()
            .uri("/")
            .header("x-real-ip", "198.51.100.2")
            .body(Body::empty())
            .expect("request");

        let parsed = parse_real_ip_header(
            request.headers().get(X_REAL_IP).expect("no header"),
            "192.0.2.5",
        )
        .expect("present");
        assert_eq!(parsed, IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2)));
    }

    #[test]
    fn parse_real_ip_header_ignores_invalid_ip() {
        let request = Request::builder()
            .uri("/")
            .header("x-real-ip", "not-an-ip")
            .body(Body::empty())
            .expect("request");

        assert!(
            parse_real_ip_header(
                request.headers().get(X_REAL_IP).expect("no header"),
                "192.0.2.5"
            )
            .is_none()
        );
    }

    #[test]
    fn parse_forwarded_header_accepts_for_value() {
        let request = Request::builder()
            .uri("/")
            .header("forwarded", "for=198.51.100.2;proto=https;by=203.0.113.10")
            .body(Body::empty())
            .expect("request");

        let parsed = parse_forwarded_header(request.headers().get("forwarded").expect("no header"))
            .expect("parse result")
            .expect("ip present");
        assert_eq!(parsed, IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2)));
    }

    #[tokio::test]
    async fn request_logger_strips_invalid_forwarded_headers() {
        async fn observed_headers(headers: axum::http::HeaderMap) -> StatusCode {
            if headers.contains_key("x-forwarded-for") || headers.contains_key("x-real-ip") {
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                StatusCode::OK
            }
        }

        let app = Router::new()
            .route("/", get(observed_headers))
            .layer(axum::middleware::from_fn(request_logger));

        let request = Request::builder()
            .uri("/")
            .header("x-forwarded-for", "203.0.113.1, nope")
            .header("x-real-ip", "not-an-ip")
            .body(Body::empty())
            .expect("request");

        let response = app.oneshot(request).await.expect("send request");
        assert_eq!(response.status(), StatusCode::OK);
    }
}
