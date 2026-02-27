use std::io;
use std::result::Result;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use inkdrip_core::error::InkDripError;

/// API error wrapper that converts `InkDripError` into HTTP responses.
pub struct ApiError(pub InkDripError);

impl From<InkDripError> for ApiError {
    fn from(err: InkDripError) -> Self {
        Self(err)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self(InkDripError::Other(err))
    }
}

impl From<io::Error> for ApiError {
    fn from(err: io::Error) -> Self {
        Self(InkDripError::Io(err))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self.0 {
            InkDripError::BookNotFound(_) | InkDripError::FeedNotFound(_) => {
                (StatusCode::NOT_FOUND, self.0.to_string())
            }
            InkDripError::Unauthorized => (StatusCode::UNAUTHORIZED, self.0.to_string()),
            InkDripError::AmbiguousId(_) => (StatusCode::BAD_REQUEST, self.0.to_string()),
            InkDripError::DuplicateBook(_) => (StatusCode::CONFLICT, self.0.to_string()),
            InkDripError::UnsupportedFormat(_) | InkDripError::ConfigError(_) => {
                (StatusCode::BAD_REQUEST, self.0.to_string())
            }
            InkDripError::ParseError(_) | InkDripError::SplitError(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, self.0.to_string())
            }
            InkDripError::StorageError(_) => {
                tracing::error!("Storage error: {}", self.0);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal storage error".to_owned(),
                )
            }
            InkDripError::Io(_) => {
                tracing::error!("IO error: {}", self.0);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal IO error".to_owned(),
                )
            }
            InkDripError::Other(_) => {
                tracing::error!("Unexpected error: {}", self.0);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal error".to_owned(),
                )
            }
        };

        let body = json!({
            "error": message,
        });

        (status, axum::Json(body)).into_response()
    }
}

/// Convenience type alias for route handler results.
pub type ApiResult<T> = Result<T, ApiError>;
