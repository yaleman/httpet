use rand::Rng;
use rand::distr::Alphanumeric;
use tower_sessions::Session;

use crate::error::HttpetError;

const CSRF_TOKEN_KEY: &str = "csrf_token";

fn generate_token() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

pub(crate) async fn csrf_token(session: &Session) -> Result<String, HttpetError> {
    let existing = session
        .get::<String>(CSRF_TOKEN_KEY)
        .await
        .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
    let token = existing.unwrap_or_else(generate_token);
    session
        .insert(CSRF_TOKEN_KEY, token.clone())
        .await
        .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
    Ok(token)
}

pub(crate) async fn validate_csrf(session: &Session, token: &str) -> Result<(), HttpetError> {
    let stored = session
        .get::<String>(CSRF_TOKEN_KEY)
        .await
        .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
    match stored {
        Some(expected) if expected == token => Ok(()),
        _ => Err(HttpetError::Unauthorized),
    }
}
