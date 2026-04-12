use crate::{handlers::ApiError, AppState};
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use shared_core::models::SegmentationJob;
use std::{path::Path, sync::Arc};


// The payload Cloudflare Worker will send
#[derive(Deserialize)]
pub struct R2WebhookPayload {
    pub file_name: String,
    pub size_bytes: u64,
}



/// POST /api/webhook/r2
pub async fn handle_r2_upload(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<R2WebhookPayload>,
) -> Result<&'static str, ApiError> {
    
    // Safely extract the ULID regardless of the file extension (.mp4, .mov, .mkv, etc.)
    let video_id = Path::new(&payload.file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| {
            ApiError::BadRequest("Invalid file name: missing base name".to_string())
        })?
        .to_string();

    tracing::info!(
        video_id = %video_id, 
        size = %payload.size_bytes, 
        "Webhook received: R2 upload complete."
    );

    // Create the job payload for our background workers
    let job = SegmentationJob {
        video_id: video_id.clone(),
        file_name: payload.file_name.clone(),
    };
    
    // Serialize it to a raw JSON string
    let job_string = serde_json::to_string(&job)
        .map_err(|e| ApiError::Internal(format!("Failed to serialize job: {}", e)))?;

    // Push it to SQS!
    state.queue.push_job(
        "http://localhost:4566/000000000000/segmentation-queue", 
        &job_string
    ).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    tracing::info!(video_id = %video_id, "Successfully queued SegmentationJob");

    // Return a simple 200 OK
    Ok("Webhook processed")
}
