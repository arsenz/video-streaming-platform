use anyhow::{Context, Result};
use shared_core::{
    db::DatabaseClient,
    infra::CoreInfrastructure,
    models::{SegmentationJob, TranscodeJob},
    queue::{JobQueue, SqsQueue},
    storage::StorageClient,
};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const RESOLUTIONS: [&str; 3] = ["1080p", "720p", "480p"];

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize Tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "worker_segmentation=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Segmentation Worker...");

    // Initialize Infrastructure
    let infra = CoreInfrastructure::load_defaults().await;

    //  Build the exact queue URLs dynamically
    let segmentation_queue_url = format!("{}/segmentation-queue", infra.queue_base_url);
    let _transcode_queue_url = format!("{}/transcode-queue", infra.queue_base_url);

    // The Infinite Polling Loop
    loop {
        match infra.queue.pull_job(&segmentation_queue_url).await {
            Ok(Some((payload, receipt_handle))) => {
                let job: SegmentationJob = match serde_json::from_str(&payload) {
                    Ok(j) => j,
                    Err(e) => {
                        error!("Poison pill received (bad JSON): {}. Deleting.", e);
                        let _ = infra
                            .queue
                            .ack_job(&segmentation_queue_url, &receipt_handle)
                            .await;
                        continue;
                    }
                };

                info!(video_id = %job.video_id, "Picked up segmentation job");

                // Segment the video
                match process_video(
                    &job,
                    &infra.db,
                    &infra.queue,
                    &infra.storage,
                    &infra.queue_base_url,
                )
                .await
                {
                    Ok(_) => {
                        info!(video_id = %job.video_id, "Segmentation complete. Acking message.");
                        infra
                            .queue
                            .ack_job(&segmentation_queue_url, &receipt_handle)
                            .await?;
                    }
                    Err(e) => {
                        error!(video_id = %job.video_id, "Failed to segment video: {:#}", e);
                        // Do NOT ack. Let the visibility timeout expire so it retries.
                    }
                }
            }
            Ok(None) => {
                // Long polling returned empty, loop again
                continue;
            }
            Err(e) => {
                error!("Failed to pull from queue: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn process_video(
    job: &SegmentationJob,
    db: &DatabaseClient,
    queue: &SqsQueue,
    storage: &StorageClient,
    queue_base_url: &str,
) -> Result<()> {
    // Create a temporary working directory for this video
    let work_dir = format!("/tmp/spart_video_{}", job.video_id);
    tokio::fs::create_dir_all(&work_dir).await?;

    let result = do_process_video(job, db, queue, storage, queue_base_url, &work_dir).await;

    // Cleanup local temp files ALWAYS regardless of success or failure
    tokio::fs::remove_dir_all(&work_dir).await.ok();

    result
}

async fn do_process_video(
    job: &SegmentationJob,
    db: &DatabaseClient,
    queue: &SqsQueue,
    storage: &StorageClient,
    queue_base_url: &str,
    work_dir: &str,
) -> Result<()> {
    let input_path = format!("{}/input_file", work_dir);
    let output_pattern = format!("{}/segment_%03d.ts", work_dir);

    //  Download original file from storage (Cloudflare R2 mock)
    info!(video_id = %job.video_id, "Downloading original video from storage...");
    storage
        .download_file(&job.file_name, &input_path)
        .await
        .context("Failed to download original video")?;

    //  Execute FFmpeg ensuring GOP at I-keyframes
    info!(video_id = %job.video_id, "Starting FFmpeg segmentation...");

    let output = tokio::process::Command::new("ffmpeg")
        .arg("-i")
        .arg(&input_path)
        // Force GOP (Group of Pictures) size to 48 frames, creating I-keyframes exactly at those boundaries
        .arg("-g")
        .arg("48")
        .arg("-keyint_min")
        .arg("48")
        .arg("-sc_threshold")
        .arg("0") // Disable scene change detection so chunks are strictly uniform
        .arg("-f")
        .arg("segment")
        .arg("-segment_time")
        .arg("4") // 4-second segments
        .arg("-c:v")
        .arg("libx264")
        .arg("-reset_timestamps")
        .arg("1")
        .arg(&output_pattern)
        .output()
        .await
        .context("Failed to execute FFmpeg")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("FFmpeg failed: {}", stderr);
    }

    // Count the generated segments
    let mut entries = tokio::fs::read_dir(&work_dir).await?;
    let mut segments = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.ends_with(".ts") {
            segments.push(file_name);
        }
    }

    let total_segments = segments.len() as u32;
    if total_segments == 0 {
        anyhow::bail!("FFmpeg completed but zero segments were found");
    }

    info!(video_id = %job.video_id, count = total_segments, "Video segmented successfully");

    // Update DB with the total number of individual transcode jobs
    db.set_total_segments(&job.video_id, total_segments).await?;

    // Fan-out to the Transcode Queues
    for segment in segments {
        let local_file_path = format!("{}/{}", work_dir, segment);
        let r2_object_key = format!("segments/{}/{}", job.video_id, segment);

        storage
            .upload_file(&r2_object_key, &local_file_path)
            .await
            .context(format!("Failed to upload segment {}", segment))?;

        for resolution in &RESOLUTIONS {
            let transcode_job = TranscodeJob {
                video_id: job.video_id.clone(),
                segment_name: r2_object_key.clone(),
                resolution: resolution.to_string(),
            };

            let payload = serde_json::to_string(&transcode_job)?;

            // Route lower resolutions to the HIGH priority queue
            let target_queue = if *resolution == "480p" || *resolution == "360p" {
                format!("{}/transcode-queue-high", queue_base_url)
            } else {
                format!("{}/transcode-queue-low", queue_base_url)
            };

            queue.push_job(&target_queue, &payload).await?;
        }
    }

    Ok(())
}
