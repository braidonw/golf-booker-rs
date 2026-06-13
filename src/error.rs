//! Application-wide error type for request handlers.

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

/// Wraps any error and renders it as an HTTP response. Server-side failures
/// default to `500`; use [`AppError::not_found`] for `404`s. Lets handlers use
/// `?` instead of panicking.
pub struct AppError {
    status: StatusCode,
    source: anyhow::Error,
}

impl AppError {
    /// A `404 Not Found` carrying the given cause.
    pub fn not_found(source: impl Into<anyhow::Error>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            source: source.into(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            tracing::error!("request error ({}): {:#}", self.status, self.source);
        } else {
            tracing::warn!("request error ({}): {:#}", self.status, self.source);
        }

        let body = Html(format!(
            "<p>{}</p>",
            html_escape::encode_text(&self.source.to_string())
        ));
        (self.status, body).into_response()
    }
}

// Allow `?` on anything convertible into `anyhow::Error`; these become `500`s.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            source: err.into(),
        }
    }
}
