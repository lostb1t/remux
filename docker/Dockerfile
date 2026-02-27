FROM jellyfin/jellyfin:10.11.6 AS jf

FROM lukemathwalker/cargo-chef:latest-rust-slim-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        curl ca-certificates \
        clang pkg-config \
        ffmpeg libavutil-dev libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev libswresample-dev libswscale-dev \
        build-essential \
        libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Install dioxus CLI and wasm target for dashboard build
RUN rustup target add wasm32-unknown-unknown \
    && cargo install dioxus-cli --locked

# Dep-layer: only invalidated when Cargo.lock/Cargo.toml/vendor changes
COPY vendor ./vendor
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Source + build — only user crates compiled here
COPY . .

# Build dashboard WASM (outputs to crates/dashboard/dist/)
RUN cd crates/dashboard && dx build --release

ENV SQLX_OFFLINE=true
RUN cargo build --release -p remux-server

RUN strip target/release/remux-server

FROM debian:bookworm-slim AS release
ENV WEB_PATH=/app/jellyfin-web
ENV DASHBOARD_PATH=/app/dashboard

RUN apt update \
    && apt install -y ffmpeg \
    && apt clean \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

WORKDIR /app
COPY --from=builder /app/target/release/remux-server .
COPY --from=builder /app/crates/dashboard/dist /app/dashboard
COPY --from=jf /jellyfin/jellyfin-web ${WEB_PATH}

EXPOSE 3000

CMD ["./remux-server"]
