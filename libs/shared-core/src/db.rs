use aws_config::meta::region::RegionProviderChain;
use aws_sdk_dynamodb::{Client, types::{AttributeValue, ReturnValue}};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DatabaseError {
    #[error("Failed to create video record: {0}")]
    CreateFailed(String),

    #[error("Failed to update video status: {0}")]
    UpdateFailed(String),

    #[error("Failed to fetch video record: {0}")]
    FetchFailed(String),
    
    #[error("Video not found: {0}")]
    NotFound(String),
}

// We define an enum for strict status typing
#[derive(Debug, PartialEq)]
pub enum VideoStatus {
    Pending,
    Processing,
    Ready,
}

impl VideoStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            VideoStatus::Pending => "pending",
            VideoStatus::Processing => "processing",
            VideoStatus::Ready => "ready",
        }
    }
}

pub struct DatabaseClient {
    client: Client,
    table_name: String,
}

impl DatabaseClient {
    pub async fn new(endpoint_url: Option<String>, table_name: &str) -> Self {
        let region_provider = RegionProviderChain::default_provider().or_else("us-east-1");

        let mut config_builder = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(region_provider);

        if let Some(url) = endpoint_url {
            config_builder = config_builder.endpoint_url(url);
        }

        let config = config_builder.load().await;

        Self {
            client: Client::new(&config),
            table_name: table_name.to_string(),
        }
    }

    /// Creates the initial record when the user requests an upload URL
    pub async fn create_video(&self, video_id: &str) -> Result<(), DatabaseError> {
        self.client
            .put_item()
            .table_name(&self.table_name)
            .item("video_id", AttributeValue::S(video_id.to_string()))
            .item("status", AttributeValue::S(VideoStatus::Pending.as_str().to_string()))
            .item("created_at", AttributeValue::N(chrono::Utc::now().timestamp().to_string()))
            .send()
            .await
            .map_err(|e| DatabaseError::CreateFailed(e.to_string()))?;

        Ok(())
    }

    /// Used by the workers to advance the state machine
    pub async fn update_status(&self, video_id: &str, new_status: VideoStatus) -> Result<(), DatabaseError> {
        self.client
            .update_item()
            .table_name(&self.table_name)
            .key("video_id", AttributeValue::S(video_id.to_string()))
            // 'status' is a reserved word in DynamoDB, so we use expression attribute names (#s)
            .update_expression("SET #s = :new_status")
            .expression_attribute_names("#s", "status")
            .expression_attribute_values(":new_status", AttributeValue::S(new_status.as_str().to_string()))
            .send()
            .await
            .map_err(|e| DatabaseError::UpdateFailed(e.to_string()))?;

        Ok(())
    }

    /// Used by the API server when client polls for status updates
    pub async fn get_status(&self, video_id: &str) -> Result<String, DatabaseError> {
        let response = self.client
            .get_item()
            .table_name(&self.table_name)
            .key("video_id", AttributeValue::S(video_id.to_string()))
            .send()
            .await
            .map_err(|e| DatabaseError::FetchFailed(e.to_string()))?;

        let item = response.item().ok_or_else(|| DatabaseError::NotFound(video_id.to_string()))?;
        
        let status_attr = item.get("status").ok_or_else(|| DatabaseError::FetchFailed("Missing status field".into()))?;
        
        if let AttributeValue::S(status_str) = status_attr {
            Ok(status_str.clone())
        } else {
            Err(DatabaseError::FetchFailed("Status is not a string".into()))
        }
    }

    /// Called by the Segmentation Worker once it knows how many chunks exist.
    /// It sets the total, initializes processed to 0, and flips status to Processing.
    pub async fn set_total_segments(&self, video_id: &str, total: u32) -> Result<(), DatabaseError> {
        self.client
            .update_item()
            .table_name(&self.table_name)
            .key("video_id", AttributeValue::S(video_id.to_string()))
            .update_expression("SET total_segments = :total, processed_segments = :zero, #s = :status")
            .expression_attribute_names("#s", "status")
            .expression_attribute_values(":total", AttributeValue::N(total.to_string()))
            .expression_attribute_values(":zero", AttributeValue::N("0".to_string()))
            .expression_attribute_values(":status", AttributeValue::S(VideoStatus::Processing.as_str().to_string()))
            .send()
            .await
            .map_err(|e| DatabaseError::UpdateFailed(format!("Failed to set segments: {}", e)))?;

        Ok(())
    }

    /// Called by Transcode Workers. 
    /// Atomically increments the processed count. Returns `true` if this was the final segment.
    pub async fn increment_processed(&self, video_id: &str) -> Result<bool, DatabaseError> {
        let response = self.client
            .update_item()
            .table_name(&self.table_name)
            .key("video_id", AttributeValue::S(video_id.to_string()))
            // The ADD keyword natively performs an atomic increment on numeric fields
            .update_expression("ADD processed_segments :one")
            .expression_attribute_values(":one", AttributeValue::N("1".to_string()))
            // We ask DynamoDB to return the new state of the row after the math is done
            .return_values(ReturnValue::AllNew) 
            .send()
            .await
            .map_err(|e| DatabaseError::UpdateFailed(format!("Failed to increment: {}", e)))?;

        // Evaluate if the video is fully processed
        if let Some(attributes) = response.attributes() {
            let processed = extract_u32(attributes, "processed_segments");
            let total = extract_u32(attributes, "total_segments");

            if processed > 0 && processed == total {
                return Ok(true); // All segments are done!
            }
        }

        Ok(false)
    }
}
// A quick helper function to parse DynamoDB numbers safely
fn extract_u32(attributes: &std::collections::HashMap<String, AttributeValue>, key: &str) -> u32 {
    if let Some(AttributeValue::N(num_str)) = attributes.get(key) {
        num_str.parse::<u32>().unwrap_or(0)
    } else {
        0
    }
}