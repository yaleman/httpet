//! Error handling

use axum::response::{IntoResponse, Redirect};
use tracing::info;

/// definitions for the httpet application.
#[derive(Debug)]
pub enum HttpetError {
    /// When you didn't do the right thing
    BadRequest,
    /// Missing or invalid session
    Unauthorized,
    /// When DB operations fail
    DatabaseError(sea_orm::DbErr),
    /// When a requested resource is not found
    NotFound(String),
    /// When an internal server error occurs
    InternalServerError(String),

    /// NeedsVote
    NeedsVote(String, String),
}

impl From<sea_orm::DbErr> for HttpetError {
    fn from(err: sea_orm::DbErr) -> Self {
        HttpetError::DatabaseError(err)
    }
}

impl From<std::io::Error> for HttpetError {
    fn from(err: std::io::Error) -> Self {
        HttpetError::InternalServerError(err.to_string())
    }
}

impl From<axum::http::Error> for HttpetError {
    fn from(err: axum::http::Error) -> Self {
        HttpetError::InternalServerError(err.to_string())
    }
}

impl From<url::ParseError> for HttpetError {
    fn from(err: url::ParseError) -> Self {
        HttpetError::InternalServerError(err.to_string())
    }
}

impl IntoResponse for HttpetError {
    fn into_response(self) -> axum::response::Response {
        match self {
            HttpetError::NeedsVote(base_url, animal) => {
                Redirect::to(&format!("{}/vote/{}", base_url, animal)).into_response()
            }
            HttpetError::BadRequest => {
                info!("Bad request received");
                let mut response =
                    axum::response::Response::new(axum::body::Body::from("Bad Request"));
                *response.status_mut() = axum::http::StatusCode::BAD_REQUEST;
                response
            }
            HttpetError::Unauthorized => {
                info!("Unauthorized request received");
                let mut response = axum::response::Response::new(axum::body::Body::from(
                    "Unauthorized: invalid or missing session.",
                ));
                *response.status_mut() = axum::http::StatusCode::UNAUTHORIZED;
                response
            }
            HttpetError::DatabaseError(err) => {
                tracing::error!("Database error: {}", err);
                let mut response =
                    axum::response::Response::new(axum::body::Body::from("Database error"));
                *response.status_mut() = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                response
            }
            HttpetError::NotFound(url) => {
                tracing::error!("404 {url}");
                let mut response =
                    axum::response::Response::new(axum::body::Body::from("Not Found"));
                *response.status_mut() = axum::http::StatusCode::NOT_FOUND;
                response
            }
            HttpetError::InternalServerError(message) => {
                tracing::error!("Internal server error: {}", message);
                let mut response =
                    axum::response::Response::new(axum::body::Body::from("Internal server error"));
                *response.status_mut() = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                response
            }
        }
    }
}
