// services/api-server/src/handlers/upload.rs
use crate::{AppState};
use axum::{extract::State, Json};
use serde::Serialize;
use ulid::Ulid;
use std::sync::Arc;
use crate::{handlers::ApiError};

// The JSON response we will send back to Svelte
#[derive(Serialize)]
pub struct UploadResponse {
    pub video_id: String,
    pub upload_url: String,
}

/// GET /api/upload/url
pub async fn generate_upload_url(
    State(state): State<Arc<AppState>>,
) -> Result<Json<UploadResponse>, ApiError> {
    
    // Generate a unique ID and filename
    let video_id = Ulid::new().to_string();
    let file_name = format!("{}.mp4", video_id);

    //  Ask S3/R2 for a Presigned PUT URL (valid for 15 minutes)
    let upload_url = state
        .storage
        .generate_upload_url(&file_name, 900)
        .await?;

    // Create the initial Database record (Status defaults to "Pending")
    state.db.create_video(&video_id).await?;

    //  Return the data to the frontend
    Ok(Json(UploadResponse {
        video_id,
        upload_url,
    }))
}
