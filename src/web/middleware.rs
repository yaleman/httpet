use axum::extract::FromRequestParts;
use axum::http::header::HOST;
use axum::http::request::Parts;
use axum::body::Body;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{Redirect, Response};

use super::prelude::*;
use super::{AppState, normalize_pet_name};

#[derive(Debug, Clone)]
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

fn normalize_host(host: &str) -> String {
    host.split(':')
        .next()
        .unwrap_or(host)
        .trim_end_matches('.')
        .to_ascii_lowercase()
}
