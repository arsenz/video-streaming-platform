// services/api-server/src/main.rs
use shared_core::{db::DatabaseClient, queue::{JobQueue, SqsQueue}, storage::StorageClient};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use std::{env, sync::Arc};
use tower_http::cors::CorsLayer;

pub mod handlers;
mod router;

// This struct holds our infrastructure clients. 
// Axum will pass an Arc reference of this to every endpoint.
pub struct AppState {
    pub db: DatabaseClient,
    pub storage: StorageClient,
    pub queue: Arc<dyn JobQueue>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Default to LocalStack, but allow production to pass a real AWS/R2 endpoint
    let aws_endpoint = env::var("AWS_ENDPOINT_URL")
        .unwrap_or_else(|_| "http://localhost:4566".to_string());
    // This looks for the RUST_LOG environment variable, or defaults to "info" level
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "api_server=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting up the API server...");
    // Default to local port 3000, but allow cloud hosts (like Fly.io or Heroku) to assign a port
    let port = env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string());
        
    let db_table_name = env::var("DYNAMODB_TABLE")
        .unwrap_or_else(|_| "videos".to_string());

    // Pass these configs to infrastructure clients
    let db_client = DatabaseClient::new(Some(aws_endpoint.clone()), &db_table_name).await;
    let storage_client = StorageClient::new(&aws_endpoint, "video-uploads").await;
    
    let sqs_client = SqsQueue::new(Some(aws_endpoint)).await;
    let state = Arc::new(AppState {
        db: db_client,
        storage: storage_client,
        queue: Arc::new(sqs_client)
    });

    // Build the Axum Router
    let app = router::build_router(state)
        // Allow Svelte (usually runs on port 5173) to call this API
        .layer(CorsLayer::permissive());

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("API Server running on http://{}", addr);
    axum::serve(listener, app).await?;



    Ok(())
}