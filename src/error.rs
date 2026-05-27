use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error as ThisError;

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("authentication error: {0}")]
    Authentication(String),
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("server error: {0}")]
    Server(#[from] axum::Error),
}

impl Error {
    pub fn anthropic_type(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) | Self::Json(_) => "invalid_request_error",
            Self::Authentication(_) => "authentication_error",
            Self::Upstream(_)
            | Self::Network(_)
            | Self::Io(_)
            | Self::Database(_)
            | Self::Server(_) => "api_error",
            Self::Config(_) => "api_error",
        }
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::InvalidRequest(_) | Self::Json(_) => StatusCode::BAD_REQUEST,
            Self::Authentication(_) => StatusCode::UNAUTHORIZED,
            Self::Upstream(_)
            | Self::Network(_)
            | Self::Io(_)
            | Self::Database(_)
            | Self::Server(_) => {
                StatusCode::BAD_GATEWAY
            }
            Self::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = AnthropicErrorBody {
            r#type: "error",
            error: AnthropicError {
                r#type: self.anthropic_type(),
                message: self.to_string(),
            },
        };
        (status, Json(body)).into_response()
    }
}

#[derive(Serialize)]
pub struct AnthropicErrorBody {
    pub r#type: &'static str,
    pub error: AnthropicError,
}

#[derive(Serialize)]
pub struct AnthropicError {
    pub r#type: &'static str,
    pub message: String,
}

pub fn anthropic_error_response(
    status: StatusCode,
    error_type: &'static str,
    message: impl Into<String>,
) -> Response {
    (
        status,
        Json(AnthropicErrorBody {
            r#type: "error",
            error: AnthropicError {
                r#type: error_type,
                message: message.into(),
            },
        }),
    )
        .into_response()
}
