use anyhow::{Context, Result};
use shared_core::{
    db::VideoStatus,
    infra::CoreInfrastructure,
    models::PlaylistJob,
    queue::JobQueue,
};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const RESOLUTIONS: [&str; 4] = ["1080p", "720p", "480p", "360p"];

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "worker_playlist=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Playlist Worker...");

    let infra = CoreInfrastructure::load_defaults().await;
    let playlist_queue_url = format!("{}/playlist-queue.fifo", infra.queue_base_url);

    loop {
        match infra.queue.pull_job(&playlist_queue_url).await {
            Ok(Some((payload, receipt_handle))) => {
                let job: PlaylistJob = match serde_json::from_str(&payload) {
                    Ok(j) => j,
                    Err(e) => {
                        error!("Poison pill received: {}. Deleting.", e);
                        let _ = infra.queue.ack_job(&playlist_queue_url, &receipt_handle).await;
                        continue;
                    }
                };

                info!(video_id = %job.video_id, res = %job.res, "Picked up playlist job");

                match process_playlist(&job, &infra).await {
                    Ok(_) => {
                        infra.queue.ack_job(&playlist_queue_url, &receipt_handle).await.ok();
                        info!(video_id = %job.video_id, res = %job.res, "Playlist processed successfully");
                    }
                    Err(e) => {
                        error!(video_id = %job.video_id, res = %job.res, "Failed to process playlist: {:#}", e);
                    }
                }
            }
            Ok(None) => continue,
            Err(e) => {
                error!("Failed to pull from queue: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn process_playlist(job: &PlaylistJob, infra: &CoreInfrastructure) -> Result<()> {
    let work_dir = format!("/tmp/playlist_{}_{}", job.video_id, job.res);
    tokio::fs::create_dir_all(&work_dir).await?;

    let result = do_process_playlist(job, infra, &work_dir).await;

    tokio::fs::remove_dir_all(&work_dir).await.ok();

    result
}

async fn do_process_playlist(job: &PlaylistJob, infra: &CoreInfrastructure, work_dir: &str) -> Result<()> {
    // 1. Generate Variant Playlist
    let mut variant_content = String::new();
    variant_content.push_str("#EXTM3U\n");
    variant_content.push_str("#EXT-X-VERSION:3\n");
    variant_content.push_str("#EXT-X-TARGETDURATION:4\n");
    variant_content.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");

    for i in 0..job.segment_count {
        variant_content.push_str("#EXTINF:4.000000,\n");
        variant_content.push_str(&format!("segment_{:03}.ts\n", i));
    }
    variant_content.push_str("#EXT-X-ENDLIST\n");

    let variant_path = format!("{}/playlist.m3u8", work_dir);
    tokio::fs::write(&variant_path, variant_content).await?;

    let variant_r2_key = format!("transcoded/{}/{}/playlist.m3u8", job.video_id, job.res);
    infra.storage.upload_file(&variant_r2_key, &variant_path).await?;
    info!(video_id = %job.video_id, res = %job.res, "Uploaded variant playlist");

    // 2. Query DB to check what resolutions are fully processed
    let (total_segments, processed_counts) = infra.db.get_video_stats(&job.video_id).await?;

    // 3. Generate Master Playlist
    let mut master_content = String::new();
    master_content.push_str("#EXTM3U\n");

    let mut fully_processed = Vec::new();

    for res in RESOLUTIONS {
        if let Some(&count) = processed_counts.get(res) {
            if count > 0 && count == total_segments {
                fully_processed.push(res);
                
                let (bandwidth, resolution) = match res {
                    "1080p" => ("5000000", "1920x1080"),
                    "720p"  => ("2800000", "1280x720"),
                    "480p"  => ("1400000", "854x480"),
                    "360p"  => ("800000", "640x360"),
                    _ => ("1000000", "0x0"),
                };

                master_content.push_str(&format!(
                    "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={}\n",
                    bandwidth, resolution
                ));
                master_content.push_str(&format!("{}/playlist.m3u8\n", res));
            }
        }
    }

    let master_path = format!("{}/master.m3u8", work_dir);
    tokio::fs::write(&master_path, master_content).await?;

    let master_r2_key = format!("transcoded/{}/master.m3u8", job.video_id);
    infra.storage.upload_file(&master_r2_key, &master_path).await?;
    info!(video_id = %job.video_id, variants = ?fully_processed, "Uploaded master playlist");

    // 4. Update Status to Ready
    infra.db.update_status(&job.video_id, VideoStatus::Ready).await?;
    info!(video_id = %job.video_id, "Video status set to Ready");

    Ok(())
}
