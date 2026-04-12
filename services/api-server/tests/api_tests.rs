use reqwest::Client;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::time::sleep;
// Helper function to find the compiled binary in the target/debug directory
fn get_binary_path(bin_name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("Failed to get current executable path");
    path.pop(); // Removes the currently running test binary name
    path.pop(); // Removes the 'deps' directory
    path.join(bin_name) 
}

struct TestEnvironment {
    api_server: Child,
    segment_worker: Child,
    transcode_worker: Child,
    playlist_worker: Child,
}

impl TestEnvironment {
    fn new() -> Self {
        println!("Setting up E2E environment...");

        // We removed the `cargo build` command from here. 
        // Cargo handles compilation before running the test!

        let api_server = Command::new(get_binary_path("api-server"))
            .current_dir("../../")
            .env("PORT", "3003")
            .env("AWS_ENDPOINT_URL", "http://localhost:4566")
            .env("R2_ENDPOINT_URL", "http://localhost:4566")
            .env("DYNAMODB_TABLE", "videos")
            .env("AWS_ACCESS_KEY_ID", "test")
            .env("AWS_SECRET_ACCESS_KEY", "test")
            .env("AWS_REGION", "us-east-1")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Failed to start api-server");

        let segment_worker = Command::new(get_binary_path("worker-segment"))
            .current_dir("../../")
            .env("AWS_ENDPOINT_URL", "http://localhost:4566")
            .env("R2_ENDPOINT_URL", "http://localhost:4566")
            .env("QUEUE_BASE_URL", "http://localhost:4566/000000000000")
            .env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", std::env::var("PATH").unwrap_or_default()))
            .env("AWS_ACCESS_KEY_ID", "test")
            .env("AWS_SECRET_ACCESS_KEY", "test")
            .env("AWS_REGION", "us-east-1")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Failed to start segmentation worker");

        let transcode_worker = Command::new(get_binary_path("worker-transcode"))
            .current_dir("../../")
            .env("AWS_ENDPOINT_URL", "http://localhost:4566")
            .env("R2_ENDPOINT_URL", "http://localhost:4566")
            .env("QUEUE_BASE_URL", "http://localhost:4566/000000000000")
            .env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", std::env::var("PATH").unwrap_or_default()))
            .env("AWS_ACCESS_KEY_ID", "test")
            .env("AWS_SECRET_ACCESS_KEY", "test")
            .env("AWS_REGION", "us-east-1")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Failed to start transcode worker");

        let playlist_worker = Command::new(get_binary_path("worker-playlist"))
            .current_dir("../../")
            .env("AWS_ENDPOINT_URL", "http://localhost:4566")
            .env("R2_ENDPOINT_URL", "http://localhost:4566")
            .env("QUEUE_BASE_URL", "http://localhost:4566/000000000000")
            .env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", std::env::var("PATH").unwrap_or_default()))
            .env("AWS_ACCESS_KEY_ID", "test")
            .env("AWS_SECRET_ACCESS_KEY", "test")
            .env("AWS_REGION", "us-east-1")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Failed to start playlist worker");

        Self {
            api_server,
            segment_worker,
            transcode_worker,
            playlist_worker,
        }
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        println!("Tearing down E2E environment...");
        self.api_server.kill().ok();
        self.segment_worker.kill().ok();
        self.transcode_worker.kill().ok();
        self.playlist_worker.kill().ok();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_pipeline_e2e_test() {
    // 0. Spin up background services
    let _env = TestEnvironment::new();
    
    // Give servers 5 seconds to bind ports
    sleep(Duration::from_secs(5)).await;

    let client = Client::new();
    let api_url = "http://localhost:3003/api";

    // 1. Get Upload URL
    println!("Requesting upload URL...");
    let res = client.get(format!("{}/upload/url", api_url)).send().await.expect("Failed to connect to API");
    assert!(res.status().is_success());
    let data: serde_json::Value = res.json().await.unwrap();
    let video_id = data["video_id"].as_str().unwrap().to_string();
    let upload_url = data["upload_url"].as_str().unwrap().to_string();

    assert!(!video_id.is_empty());
    assert!(upload_url.contains("video-uploads"));

    // 2. Read the 20-second sample video
    println!("Reading sample_20s.mp4...");
    let file_bytes = tokio::fs::read("tests/fixtures/sample_20s.mp4").await.expect("Failed to read fixture");
    let file_size = file_bytes.len() as u64;

    // 3. PUT request to S3/Localstack presigned URL
    println!("Uploading video to S3 presigned URL...");
    let res = client.put(&upload_url)
        .body(file_bytes)
        .send()
        .await
        .expect("Failed to upload object");
    assert!(res.status().is_success());

    // 4. Trigger Webhook
    println!("Triggering Cloudflare Webhook simulation...");
    let webhook_payload = serde_json::json!({
        "file_name": format!("{}.mp4", video_id),
        "size_bytes": file_size
    });

    let res = client.post(format!("{}/webhook/r2", api_url))
        .json(&webhook_payload)
        .send()
        .await
        .expect("Failed to trigger webhook");
    assert!(res.status().is_success());

    // 5. Polling Loop
    println!("Polling for status 'ready'...");
    let mut is_ready = false;
    for i in 0..120 { // Max ~4 minutes wait (120 * 2s) for transcoding 20s
        sleep(Duration::from_secs(2)).await;
        let res = client.get(format!("{}/status/{}", api_url, video_id)).send().await;
        
        if let Ok(res) = res
            && res.status().is_success() {
                let status_data: serde_json::Value = res.json().await.unwrap();
                let state = status_data["status"].as_str().unwrap();
                println!("Status poll {}: {}", i, state);

                if state == "ready" {
                    is_ready = true;
                    break;
                }
            }
    }

    assert!(is_ready, "Video did not reach 'ready' state within timeout");

    println!("Pipeline completed successfully! Video is ready.");
}
