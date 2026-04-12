use crate::{handlers, AppState};
use axum::{Router, routing::{get, post}};
use std::sync::Arc;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        // Uploading
        .route("/api/upload/url", get(handlers::upload::generate_upload_url))
        .route("/api/status/:video_id", get(handlers::status::get_video_status))
        .route("/api/webhook/r2", post(handlers::webhook::handle_r2_upload))
        .with_state(state)
}