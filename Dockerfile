# 1. Fetch static ffmpeg for instant download
FROM mwader/static-ffmpeg:7.0.2 AS ffmpeg

# 2. Builder
FROM rust:latest AS builder
WORKDIR /app

# Copy the entire workspace
COPY . .

# Pass the name of the binary to build (e.g. worker-transcode)
ARG BIN_NAME

# Use BuildKit cache mounts to cache the Cargo registry, git index, and target directory.
# CRITICAL NOTE: Because `/app/target` is a cache mount, its contents are not persisted 
# to the Docker image after this RUN command finishes. Therefore, we must copy the 
# compiled binary to a permanent location (/app/bin) in the exact same step.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    set -e && \
    cargo build --release --bin ${BIN_NAME} && \
    mkdir -p /app/bin && \
    cp /app/target/release/${BIN_NAME} /app/bin/app-run

# 3. Runtime Image
FROM ubuntu:24.04
WORKDIR /app

# Install basic certificates natively, but drop the heavy ffmpeg install
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

# Copy lightweight static ffmpeg binaries directly (Takes <1s)
COPY --from=ffmpeg /ffmpeg /usr/local/bin/
COPY --from=ffmpeg /ffprobe /usr/local/bin/

# Copy the specific built binary from our permanent location in the builder stage
COPY --from=builder /app/bin/app-run /usr/local/bin/app-run

CMD ["app-run"]