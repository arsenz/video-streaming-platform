use crate::{handlers::ApiError, AppState};
use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub struct StatusResponse {
    pub video_id: String,
    pub status: String,
}

/// GET /api/status/:video_id
pub async fn get_video_status(
    State(state): State<Arc<AppState>>,
    Path(video_id): Path<String>,
) -> Result<Json<StatusResponse>, ApiError> {
    
    // Avoid spamming logs with info! in a polled endpoint
    tracing::debug!(video_id = %video_id, "Frontend polling video status");

    // This fetches the status string ("pending", "processing", or "ready")
    // If the video_id isn't in DynamoDB, it returns a 404 ApiError automatically
    let status = state.db.get_status(&video_id).await?;

    Ok(Json(StatusResponse {
        video_id,
        status,
    }))
}
