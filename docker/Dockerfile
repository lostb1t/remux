# Stage 1: Jellyfin web client
FROM jellyfin/jellyfin:10.11.5 AS jf

# Stage 2: Build Rust application
FROM rust:slim-bookworm AS builder
WORKDIR /app

# Install build dependencies
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        curl ca-certificates \
        clang pkg-config \
        ffmpeg libavutil-dev libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev libswresample-dev libswscale-dev \
        build-essential \
        libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Debug: Print FFmpeg and pkg-config info
RUN ffmpeg -version \
    && pkg-config --modversion libavutil \
    && pkg-config --modversion libavcodec \
    && pkg-config --modversion libavformat

# Set PKG_CONFIG_PATH explicitly
ENV PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig

# Copy vendor, Cargo.toml, and Cargo.lock first for caching
COPY vendor ./vendor
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to cache dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Build dependencies
RUN cargo build --release

# Copy the rest of the source code
COPY ./src ./src

# Build the full project
RUN cargo build --release

# Strip the binary to reduce size
RUN strip target/release/remux-server

# Stage 3: Final image
FROM debian:bookworm-slim AS release
ENV WEB_PATH=/app/jellyfin-web

# Install runtime dependencies
RUN apt update \
    && apt install -y ffmpeg \  # Only the runtime is needed
    && apt clean \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

WORKDIR /app
COPY --from=builder /app/target/release/remux-server .
COPY --from=jf /jellyfin/jellyfin-web ${WEB_PATH}

EXPOSE 3000

# Set the entrypoint
CMD ["./remux-server"]
