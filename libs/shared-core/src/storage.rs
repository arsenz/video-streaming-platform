use aws_config::meta::region::RegionProviderChain;
use aws_sdk_s3::{Client, presigning::PresigningConfig, primitives::ByteStream};
use std::{path::Path, time::Duration};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Failed to configure presigned URL: {0}")]
    ConfigError(String),

    #[error("Failed to generate presigned request: {0}")]
    PresignFailed(String),

    // Add these two new errors
    #[error("Failed to read file from disk: {0}")]
    FileReadFailed(String),

    #[error("Failed to upload file to S3/R2: {0}")]
    UploadFailed(String),
    #[error("Failed to download file from S3/R2: {0}")]
    DownloadFailed(String),
}

pub struct StorageClient {
    client: Client,
    bucket_name: String,
}

impl StorageClient {
    /// Initializes the client. 
    /// By passing `endpoint_url`, we can seamlessly switch between:
    /// - LocalStack: http://localhost:4566
    /// - Cloudflare R2: https://<account_id>.r2.cloudflarestorage.com
    pub async fn new(endpoint_url: &str, bucket_name: &str) -> Self {
        // R2 doesn't strictly use regions like AWS, but the AWS SDK requires one.
        // "auto" or "us-east-1" are standard dummy regions for R2.
        let region_provider = RegionProviderChain::default_provider().or_else("auto");

        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(region_provider)
            .endpoint_url(endpoint_url)
            .load()
            .await;
        // force_path_style is crucial for LocalStack and highly recommended for R2 compatibility
        let s3_config = aws_sdk_s3::config::Builder::from(&config)
            .force_path_style(true) 
            .build();

        Self {
            client: Client::from_conf(s3_config),
            bucket_name: bucket_name.to_string(),
        }
    }

    /// Generates a temporary URL that the frontend can use to upload the 1GB video directly.
    pub async fn generate_upload_url(&self, object_key: &str, expires_in_secs: u64) -> Result<String, StorageError> {
        let expires_in = Duration::from_secs(expires_in_secs);
        
        let presigning_config = PresigningConfig::expires_in(expires_in)
            .map_err(|e| StorageError::ConfigError(e.to_string()))?;

        let presigned_request = self
            .client
            .put_object()
            .bucket(&self.bucket_name)
            .key(object_key)
            .presigned(presigning_config)
            .await
            .map_err(|e| StorageError::PresignFailed(e.to_string()))?;

        // Return the raw URL string to send back to the client
        Ok(presigned_request.uri().to_string())
    }

    /// Streams file directly into Cloudflare R2 / LocalStack
    pub async fn upload_file(&self, object_key: &str, file_path: &str) -> Result<(), StorageError> {
        // ByteStream safely handles large files without loading them entirely into memory
        let body = ByteStream::from_path(Path::new(file_path))
            .await
            .map_err(|e| StorageError::FileReadFailed(e.to_string()))?;

        self.client
            .put_object()
            .bucket(&self.bucket_name)
            .key(object_key)
            .body(body)
            .send()
            .await
            .map_err(|e| StorageError::UploadFailed(e.to_string()))?;

        Ok(())
    }
    /// Streams a file from S3/R2 directly to the local disk
    pub async fn download_file(&self, object_key: &str, file_path: &str) -> Result<(), StorageError> {
        let mut response = self.client
            .get_object()
            .bucket(&self.bucket_name)
            .key(object_key)
            .send()
            .await
            .map_err(|e| StorageError::DownloadFailed(e.to_string()))?;

        let mut file = tokio::fs::File::create(file_path)
            .await
            .map_err(|e| StorageError::FileReadFailed(e.to_string()))?;

        // Stream the bytes to disk in chunks
        while let Some(bytes) = response.body.try_next().await.map_err(|e| StorageError::DownloadFailed(e.to_string()))? {
            use tokio::io::AsyncWriteExt;
            file.write_all(&bytes).await.map_err(|e| StorageError::FileReadFailed(e.to_string()))?;
        }

        Ok(())
    }
}