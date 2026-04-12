use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use shared_core::{db::DatabaseError, storage::StorageError};
use thiserror::Error;
use tracing::error; 

pub mod upload;
pub mod status;
pub mod webhook;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("The requested resource was not found")]
    NotFound,

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Internal server error: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, client_message) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, self.to_string()),
            ApiError::BadRequest(ref msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Internal(ref msg) => {
                error!(error_message = %msg, "Critical internal API error occurred");
                
                (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong".to_string())
            }
        };

        let body = Json(json!({ "error": client_message }));
        (status, body).into_response()
    }
}

impl From<DatabaseError> for ApiError {
    fn from(err: DatabaseError) -> Self {
        match err {
            // If the DB says "Not Found", tell Axum to return a 404
            DatabaseError::NotFound(_) => ApiError::NotFound,
            
            // Everything else (connection failure, write failure) is a 500
            _ => ApiError::Internal(err.to_string()),
        }
    }
}

// Convert Storage errors into API errors
impl From<StorageError> for ApiError {
    fn from(err: StorageError) -> Self {
        // Presigned URL failures are always internal server errors
        ApiError::Internal(err.to_string())
    }
}
