use async_trait::async_trait;
use thiserror::Error;
use aws_sdk_sqs::Client;

#[derive(Error, Debug)]
pub enum QueueError {
    #[error("Failed to push message to queue: {0}")]
    PushFailed(String),

    #[error("Failed to pull message from queue: {0}")]
    PullFailed(String),

    #[error("Failed to acknowledge message: {0}")]
    AckFailed(String),
    
    #[error("Queue serialization/deserialization error: {0}")]
    FormatError(String),
}

#[async_trait]
pub trait JobQueue: Send + Sync {
    /// Pushes a raw string payload into the queue
    async fn push_job(&self, queue_url: &str, payload: &str) -> Result<(), QueueError>;
    /// Pulls a message. Returns (Payload, ReceiptHandle).
    async fn pull_job(&self, queue_url: &str) -> Result<Option<(String, String)>, QueueError>;
    /// Acknowledges (deletes) the message from the queue upon successful completion
    async fn ack_job(&self, queue_url: &str, receipt_handle: &str) -> Result<(), QueueError>;
}



pub struct SqsQueue {
    client: Client,
}

impl SqsQueue {
    // We allow passing an endpoint_url to easily swap between LocalStack and Real AWS
    pub async fn new(endpoint_url: Option<String>) -> Self {
        let mut config_builder = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new("us-east-1"));

        if let Some(url) = endpoint_url {
            config_builder = config_builder.endpoint_url(url);
        }

        let config = config_builder.load().await;
        Self {
            client: Client::new(&config),
        }
    }
}

#[async_trait]
impl JobQueue for SqsQueue {
    async fn push_job(&self, queue_url: &str, payload: &str) -> Result<(), QueueError> {
        self.client
            .send_message()
            .queue_url(queue_url)
            .message_body(payload)
            .send()
            .await.map_err(|e| QueueError::PushFailed(e.to_string()))?;
        Ok(())
    }

    // Inside your SqsQueue implementation block:

    async fn pull_job(&self, queue_url: &str) -> Result<Option<(String, String)>, QueueError> {
        let response = self.client
            .receive_message()
            .queue_url(queue_url)
            .max_number_of_messages(1)
            .wait_time_seconds(20) // Long Polling
            .send()
            .await
            .map_err(|e| QueueError::PullFailed(e.to_string()))?;

       if let Some(msg) = response.messages().first() {
            let body = msg.body().unwrap_or_default().to_string();
            let receipt_handle = msg.receipt_handle().unwrap_or_default().to_string();
            return Ok(Some((body, receipt_handle)));
        }
        
        Ok(None)
    }

    async fn ack_job(&self, queue_url: &str, receipt_handle: &str) -> Result<(), QueueError> {
        self.client
            .delete_message()
            .queue_url(queue_url)
            .receipt_handle(receipt_handle)
            .send()
            .await.map_err(|e| QueueError::AckFailed(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::{atomic::{AtomicUsize, Ordering}, Mutex};


    /// An in-memory queue that mimics SQS Visibility Timeouts and Acks
    struct MockQueue {
        pending: Mutex<VecDeque<String>>,
        in_flight: Mutex<HashMap<String, String>>, // Maps receipt_handle -> payload
        receipt_counter: AtomicUsize,
    }

    impl MockQueue {
        fn new() -> Self {
            Self {
                pending: Mutex::new(VecDeque::new()),
                in_flight: Mutex::new(HashMap::new()),
                receipt_counter: AtomicUsize::new(1),
            }
        }
    }

    #[async_trait]
    impl JobQueue for MockQueue {
        async fn push_job(&self, _queue_url: &str, payload: &str) -> Result<(), QueueError> {
            self.pending.lock().unwrap().push_back(payload.to_string());
            Ok(())
        }

        async fn pull_job(&self, _queue_url: &str) -> Result<Option<(String, String)>, QueueError> {
            let mut pending = self.pending.lock().unwrap();
            
            if let Some(payload) = pending.pop_front() {
                // Generate a fake receipt handle
                let handle = self.receipt_counter.fetch_add(1, Ordering::SeqCst).to_string();
                
                // Move message from 'pending' to 'in_flight'
                self.in_flight.lock().unwrap().insert(handle.clone(), payload.clone());
                Ok(Some((payload, handle)))
            } else {
                Ok(None)
            }
        }

        async fn ack_job(&self, _queue_url: &str, receipt_handle: &str) -> Result<(), QueueError> {
            let mut in_flight = self.in_flight.lock().unwrap();
            
            // If the handle exists, remove it (successful ack). Otherwise, fail.
            if in_flight.remove(receipt_handle).is_some() {
                Ok(())
            } else {
                Err(QueueError::AckFailed("Invalid or expired receipt handle".to_string()))
            }
        }
    }

    #[tokio::test]
    async fn test_mock_queue_lifecycle() {
        let queue = MockQueue::new();
        let queue_url = "dummy_queue";
        let payload = r#"{"video_id":"123","segment_name":"chunk_0.ts"}"#;

        // 1. Queue should start empty
        let empty = queue.pull_job(queue_url).await.unwrap();
        assert!(empty.is_none());

        // 2. Push a job
        queue.push_job(queue_url, payload).await.unwrap();

        // 3. Pull the job (it should now be "in-flight")
        let (pulled_payload, receipt_handle) = queue.pull_job(queue_url).await.unwrap().unwrap();
        assert_eq!(pulled_payload, payload);

        // 4. Pulling again should return None (message is hidden waiting for ack)
        let hidden = queue.pull_job(queue_url).await.unwrap();
        assert!(hidden.is_none());

        // 5. Ack the job
        queue.ack_job(queue_url, &receipt_handle).await.unwrap();

        // 6. Acking the same job twice should fail
        let double_ack = queue.ack_job(queue_url, &receipt_handle).await;
        assert!(double_ack.is_err());
    }
}