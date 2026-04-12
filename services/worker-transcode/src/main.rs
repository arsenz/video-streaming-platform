use anyhow::{Context, Result};
use shared_core::{
    db::{DatabaseClient, VideoStatus},
    infra::CoreInfrastructure,
    models::TranscodeJob,
    queue::JobQueue,
    storage::StorageClient,
};
use std::path::Path;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const RESOLUTIONS: [&str; 3] = ["1080p", "720p", "480p"];

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize Tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "worker_transcode=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Transcode Worker...");

    // Initialize Infrastructure
    let infra = CoreInfrastructure::load_defaults().await;

    let queue_high = format!("{}/transcode-queue-high", infra.queue_base_url);
    let queue_low = format!("{}/transcode-queue-low", infra.queue_base_url);

    loop {
        // Try High Priority First
        let mut job_result = infra.queue.pull_job(&queue_high).await;
        let mut active_queue = &queue_high;

        // Fallback to Low Priority if High is empty
        if let Ok(None) = job_result {
            job_result = infra.queue.pull_job(&queue_low).await;
            active_queue = &queue_low;
        }

        match job_result {
            Ok(Some((payload, receipt_handle))) => {
                let job: TranscodeJob = serde_json::from_str(&payload).unwrap(); // handle errs

                // process_transcode() now only runs FFmpeg for job.resolution
                match process_transcode(&job, &infra).await {
                    Ok(_) => {
                        infra.queue.ack_job(active_queue, &receipt_handle).await?;
                    }
                    Err(_e) => {
                        // Do not ack the job, let it time out and retry
                    }
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

    // Create an isolated working directory for this specific chunk and resolution
    let work_dir = format!(
        "/tmp/transcode_{}_{}_{}",
        job.video_id, job.resolution, safe_segment_name
    );
    tokio::fs::create_dir_all(&work_dir).await?;

    let result = do_process_transcode(job, infra, &work_dir, &safe_segment_name).await;

    //  Cleanup local temporary files to prevent volume exhaustion ALWAYS
    tokio::fs::remove_dir_all(&work_dir).await.ok();

    result
}

async fn do_process_transcode(
    job: &TranscodeJob,
    infra: &CoreInfrastructure,
    work_dir: &str,
    safe_segment_name: &str,
) -> Result<()> {
    let input_path = format!("{}/input.ts", work_dir);

    // Download the raw segment from R2 / LocalStack
    info!(video_id = %job.video_id, "Downloading raw segment: {}", job.segment_name);
    infra
        .storage
        .download_file(&job.segment_name, &input_path)
        .await
        .context("Failed to download segment")?;

    //  Determine target resolution and bitrate
    let (scale, bitrate) = match job.resolution.as_str() {
        "1080p" => ("1920:1080", "5000k"),
        "720p" => ("1280:720", "2800k"),
        "480p" => ("854:480", "1400k"),
        "360p" => ("640:360", "800k"),
        _ => anyhow::bail!("Unsupported resolution: {}", job.resolution),
    };

    let output_path = format!("{}/{}_{}", work_dir, job.resolution, safe_segment_name);

    info!(video_id = %job.video_id, "Transcoding to {} ({})", job.resolution, scale);

    //  Execute FFmpeg for the specific format
    let output = tokio::process::Command::new("ffmpeg")
        .arg("-y") // Overwrite without prompting
        .arg("-i")
        .arg(&input_path)
        .arg("-vf")
        .arg(format!("scale={}", scale))
        .arg("-b:v")
        .arg(bitrate)
        .arg("-c:v")
        .arg("libx264")
        .arg("-c:a")
        .arg("aac") // Ensure audio is universally compatible
        .arg(&output_path)
        .output()
        .await
        .context(format!("Failed to execute FFmpeg for {}", job.resolution))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("FFmpeg failed for {}: {}", job.resolution, stderr);
    }

    //  Upload the transcoded chunk to the target CDN directory
    let r2_output_key = format!(
        "transcoded/{}/{}/{}",
        job.video_id, job.resolution, safe_segment_name
    );
    infra
        .storage
        .upload_file(&r2_output_key, &output_path)
        .await
        .context(format!(
            "Failed to upload transcoded segment {}",
            job.resolution
        ))?;

    //  Update the Database Atomically
    // This uses the ADD expression you set up to handle parallel worker completion perfectly
    let is_final_segment = infra
        .db
        .increment_processed(&job.video_id)
        .await
        .context("Failed to increment processed segment count")?;

    if let Some(total_jobs_completed) = is_final_segment {
        info!(video_id = %job.video_id, total = total_jobs_completed, "✅ Final segment transcoded! Generating HLS Playlists...");

        // Generate and upload the HLS Playlists
        generate_hls_playlists(job, infra, total_jobs_completed).await?;

        // Finally, Update Status to 'Ready'
        infra
            .db
            .update_status(&job.video_id, VideoStatus::Ready)
            .await
            .context("Failed to update video status to Ready")?;
    }

    Ok(())
}

async fn generate_hls_playlists(
    job: &TranscodeJob,
    infra: &CoreInfrastructure,
    total_jobs: u32,
) -> Result<()> {
    // The total jobs created was segments_count * number of resolutions.
    // By dividing by 3 (the number of resolutions), we get the segment count.

    let segments_count = total_jobs / (RESOLUTIONS.len() as u32);

    //  Generate Variant Playlists recursively for each resolution
    for res in &RESOLUTIONS {
        let mut playlist = String::from(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n",
        );
        for i in 0..segments_count {
            playlist.push_str(&format!("#EXTINF:4.000000,\nsegment_{:03}.ts\n", i));
        }
        playlist.push_str("#EXT-X-ENDLIST\n");

        let m3u8_key = format!("transcoded/{}/{}/playlist.m3u8", job.video_id, res);

        let temp_m3u8_path = format!("/tmp/{}_{}.m3u8", job.video_id, res);
        tokio::fs::write(&temp_m3u8_path, &playlist).await?;
        infra
            .storage
            .upload_file(&m3u8_key, &temp_m3u8_path)
            .await?;
        tokio::fs::remove_file(&temp_m3u8_path).await.ok();
    }

    //  Generate Master Playlist
    let master_m3u8 = "#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080
1080p/playlist.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2800000,RESOLUTION=1280x720
720p/playlist.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=1400000,RESOLUTION=854x480
480p/playlist.m3u8
".to_string();

    let master_key = format!("transcoded/{}/master.m3u8", job.video_id);
    let temp_master_path = format!("/tmp/{}_master.m3u8", job.video_id);
    tokio::fs::write(&temp_master_path, &master_m3u8).await?;
    infra
        .storage
        .upload_file(&master_key, &temp_master_path)
        .await?;
    tokio::fs::remove_file(&temp_master_path).await.ok();

    info!(video_id = %job.video_id, "HLS playlists generated and uploaded successfully.");
    Ok(())
}
