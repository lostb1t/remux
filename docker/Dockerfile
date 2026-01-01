FROM jellyfin/jellyfin:10.11.5 AS jf

FROM rust:slim-bookworm AS builder
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        curl ca-certificates \
        clang pkg-config \
        libavutil-dev libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev libswresample-dev libswscale-dev \
        build-essential \
        libssl-dev \  # Add libssl-dev for HTTPS support
    && rm -rf /var/lib/apt/lists/*

COPY vendor Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release

COPY ./src ./src
RUN cargo build --release

RUN strip target/release/remux-server

FROM debian:bookworm-slim AS release
ENV WEB_PATH=/app/jellyfin-web

RUN apt update \
    && apt install -y ffmpeg libavutil-dev libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev libswresample-dev libswscale-dev \
    && apt clean \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

WORKDIR /app
# Copy the Rust binary from the builder stage
COPY --from=builder /app/target/release/remux-server .
# Copy the Jellyfin web client from the web stage
COPY --from=kf /jellyfin/jellyfin-web ${WEB_PATH}

EXPOSE 3000

# Set the entrypoint
CMD ["./remux-server"]