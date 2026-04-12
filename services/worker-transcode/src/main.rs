use anyhow::{Context, Result};
use shared_core::{
    db::{DatabaseClient, VideoStatus},
    infra::CoreInfrastructure,
    models::TranscodeJob,
    queue::JobQueue,
    storage::StorageClient,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use std::path::Path;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize Tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "worker_transcode=info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Transcode Worker...");

    // Initialize Infrastructure
    let infra = CoreInfrastructure::load_defaults().await;

    let queue_high = format!("{}/transcode-queue-high", infra.queue_base_url);
    let queue_low = format!("{}/transcode-queue-low", infra.queue_base_url);

    loop {
        // 1. Try High Priority First
        let mut job_result = infra.queue.pull_job(&queue_high).await;
        let mut active_queue = &queue_high;

        // 2. Fallback to Low Priority if High is empty
        if let Ok(None) = job_result {
            job_result = infra.queue.pull_job(&queue_low).await;
            active_queue = &queue_low;
        }

        match job_result {
            Ok(Some((payload, receipt_handle))) => {
                let job: TranscodeJob = serde_json::from_str(&payload).unwrap(); // handle errs
                
                // process_transcode() now only runs FFmpeg for job.resolution
                match process_transcode(&job, &infra).await {
                    Ok(_) => { infra.queue.ack_job(active_queue, &receipt_handle).await?; }
                    Err(e) => { /* Handle error */ }
                }
            }
            Ok(None) => continue,
            Err(_) => tokio::time::sleep(tokio::time::Duration::from_secs(5)).await,
        }
    }
}

async fn process_transcode(job: &TranscodeJob, infra: &CoreInfrastructure) -> Result<()> {
    // Extract just the filename (e.g., "segment_001.ts") from the full R2 object key
    let safe_segment_name = Path::new(&job.segment_name)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    
    // Create an isolated working directory for this specific chunk
    let work_dir = format!("/tmp/transcode_{}_{}", job.video_id, safe_segment_name);
    tokio::fs::create_dir_all(&work_dir).await?;

    let input_path = format!("{}/input.ts", work_dir);
    
    // 1. Download the raw segment from R2 / LocalStack
    info!(video_id = %job.video_id, "Downloading raw segment: {}", job.segment_name);
    infra.storage.download_file(&job.segment_name, &input_path).await
        .context("Failed to download segment")?;

    // Define the target resolutions and bitrates
    // Format: (Resolution Name, Scale parameters, Video Bitrate)
    let renditions = vec![
        ("1080p", "1920:1080", "5000k"),
        ("720p", "1280:720", "2800k"),
        ("480p", "854:480", "1400k"),
    ];

    // 3. Transcode the chunk into all renditions
    for (name, scale, bitrate) in renditions {
        let output_path = format!("{}/{}_{}", work_dir, name, safe_segment_name);
        
        info!(video_id = %job.video_id, "Transcoding to {} ({})", name, scale);
        
        let output = tokio::process::Command::new("ffmpeg")
            .arg("-y") // Overwrite without prompting
            .arg("-i").arg(&input_path)
            .arg("-vf").arg(format!("scale={}", scale))
            .arg("-b:v").arg(bitrate)
            .arg("-c:v").arg("libx264")
            .arg("-c:a").arg("aac") // Ensure audio is universally compatible
            .arg(&output_path)
            .output()
            .await
            .context(format!("Failed to execute FFmpeg for {}", name))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("FFmpeg failed for {}: {}", name, stderr);
        }

        // 4. Upload the transcoded chunk to the target CDN directory
        let r2_output_key = format!("transcoded/{}/{}/{}", job.video_id, name, safe_segment_name);
        infra.storage.upload_file(&r2_output_key, &output_path).await
            .context(format!("Failed to upload transcoded segment {}", name))?;
    }

    // 5. Update the Database Atomically
    // This uses the ADD expression you set up to handle parallel worker completion perfectly
    let is_final_segment = infra.db.increment_processed(&job.video_id).await
        .context("Failed to increment processed segment count")?;

    if is_final_segment {
        info!(video_id = %job.video_id, "✅ Final segment transcoded! Marking video as Ready.");
        infra.db.update_status(&job.video_id, VideoStatus::Ready).await
            .context("Failed to update video status to Ready")?;
    }

    // 6. Cleanup local temporary files to prevent volume exhaustion
    tokio::fs::remove_dir_all(&work_dir).await?;

    Ok(())
}