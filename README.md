# Streamify 🎬

A highly-scalable, containerized, microservice-based video streaming platform built entirely in **Rust**. It utilizes a heavily parallelized FFmpeg transcode pipeline to automatically chunk uploads, asynchronously downscale into HLS adaptive quality tiers (1080p, 720p, 480p), and cleanly piece together dynamic HLS master playlists. 

All infrastructure natively runs locally over Docker Compose through **LocalStack** simulating DynamoDB, SQS, and S3 (Cloudflare R2 equivalents).

---

## 🏗️ Architecture

- **Frontend (`/frontend`)**: Glassmorphism themed Vanilla UI running on NGINX over `:8080`. Supports drag-and-drop, direct S3 uploads, and dynamic clipboard-sharing links via native URL params. Polling-based live metadata injects seamlessly into an embedded `hls.js` adaptive video player.
- **API Server (`/services/api-server`)**: Coordinates Webhooks from Cloudflare R2 / S3 mocking, exposes polling analytics (e_status), and orchestrates internal presigned URL transfers natively in Axum.
- **Worker - Segmentation (`/services/worker-segment`)**: Isolates pure MP4 uploads and rips them down strictly into 4-second `.ts` transport blocks with continuous PTS mapping. Automatically fans out parallel Transcoding Jobs into SQS.
- **Worker - Transcode (`/services/worker-transcode`)**: Horizontally scaled FFmpeg encoders! They pull segmented jobs, transcode into lower HD boundaries (`libx264/aac`), update a live atomic Database lock, and upon the absolute final segment, trigger the playlist worker.
- **Worker - Playlist (`/services/worker-playlist`)**: Rapidly rebuilds `#EXTM3U` variant files (the VOD playlist) upon exact resolution finalization, and synchronously constructs the `master.m3u8` index array to serve straight into the Frontend player natively. 

---

## 🌍 Production Deployment

The primary design principle of this architecture is **codebase immutability**. Because the underlying Rust services leverage the official AWS SDK (`aws-sdk-sqs`, `aws-sdk-dynamodb`, `aws-sdk-s3`), **you do not need to change a single line of application code to deploy to a production environment like AWS EC2.** 

To migrate from LocalStack to a live production AWS account, simply:
1. Remove or override the `AWS_ENDPOINT_URL` environment variables in your production environment config.
2. Provide standard native IAM Identity credentials or Instance Profiles allowing the AWS SDK to seamlessly fallback from the LocalStack endpoint to genuine AWS cloud regions.
3. Update specific bucket names and `PUBLIC_CDN_URL` parameters.

The exact same Rust binaries, pipeline design, and queue mechanics will transition natively!

## 🚀 Quick Start Guide

### Prerequisites
Make sure your system has the following installed:
- [Docker](https://www.docker.com/) & Docker Compose
- *Nothing else!* The entire Rust backend builds cleanly via multi-stage Dockerfiles.

### Running Locally

To orchestrate the LocalStack instances, NGINX layer, API gateway, and Transcoding Microworkers all identically synchronized:

```bash
# 1. Fire up the entire local infrastructure and detach logs
docker compose up --build

# 2. View your gorgeous client interface!
# Navigate to: http://localhost:8080/
```

### Watching Logs

Because the architecture leverages complex asynchronous fan-out processing, it is highly recommended to inspect real-time logs inside the system to watch your tasks rapidly orchestrate through SQS queues:

```bash
docker compose logs -f
```

## 📈 Scalability Settings

If you have a powerful host machine (e.g. an M-Series chip) and want to significantly boost encoding speeds on heavy files, increase your `worker-transcode` cluster internally! Because LocalStack natively models AWS visibility timeouts and we use atomic `ADD` queries into DynamoDB, you can parallelize workers safely zero-configuration!

```bash
# Spin up 4 concurrent Transcode processes in parallel purely from the CLI
docker compose up --scale worker-transcode=4 -d
```
> *(Note: A static replica declaration is already deployed inside the standard `docker-compose.yml` base script!)*

## 🧹 Tearing Down

When you are finished testing the streaming environment, gracefully shut down your Docker instances and automatically tear down the ephemeral volume containers and queues:

```bash
docker compose down -v
```


