# syntax=docker/dockerfile:1

FROM rust:1.80 AS builder
WORKDIR /app

# Copy the entire workspace
COPY . .

# Pass the name of the binary to build (e.g. worker-transcode)
ARG BIN_NAME
RUN cargo build --release --bin ${BIN_NAME}

# Runtime Image
FROM debian:bookworm-slim
WORKDIR /app

# Install runtime dependencies needed by workers (ffmpeg for transcoding/segmenting)
RUN apt-get update && apt-get install -y ffmpeg ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

# Copy the specific built binary from the builder stage
ARG BIN_NAME
COPY --from=builder /app/target/release/${BIN_NAME} /usr/local/bin/app-run

CMD ["app-run"]
