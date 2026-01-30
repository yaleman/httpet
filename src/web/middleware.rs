use axum::extract::FromRequestParts;
use axum::http::{header::HOST, request::Parts, StatusCode};

use super::AppState;

#[derive(Debug, Clone)]
pub(crate) struct AnimalDomain {
    pub(crate) host: String,
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

        Self { host, animal }
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

    Some(label.to_string())
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
