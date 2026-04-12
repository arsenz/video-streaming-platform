use crate::{db::DatabaseClient, queue::SqsQueue, storage::StorageClient};
use std::env;

pub struct CoreInfrastructure {
    pub db: DatabaseClient,
    pub storage: StorageClient,
    pub queue: SqsQueue,
    pub queue_base_url: String, // Keep this for worker queue routing
}

impl CoreInfrastructure {
    pub async fn load_defaults() -> Self {
        // AWS Services (SQS & DynamoDB)
        // If this env var is missing (like in Prod), it passes `None` to the clients,
        // and the SDK safely defaults to real AWS infrastructure.
        let aws_endpoint = env::var("AWS_ENDPOINT_URL").ok(); 
        
        // Cloudflare R2 Storage
        // R2 ALWAYS requires an endpoint. We default to LocalStack for local dev.
        let r2_endpoint = env::var("R2_ENDPOINT_URL")
            .unwrap_or_else(|_| "http://localhost:4566".to_string());

        // Initialize Clients
        let db = DatabaseClient::new(aws_endpoint.clone(), "videos").await;
        let queue = SqsQueue::new(aws_endpoint.clone()).await;
        
        // Storage Client strictly requires a string reference!
        let storage = StorageClient::new(&r2_endpoint, "video-uploads").await;

        // Queue Routing Helper
        // LocalStack uses http://localhost:4566/000000000000/queue-name
        // Real AWS uses https://sqs.us-east-1.amazonaws.com/123456789/queue-name
        let queue_base_url = env::var("QUEUE_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:4566/000000000000".to_string());

        Self {
            db,
            storage,
            queue,
            queue_base_url,
        }
    }
}