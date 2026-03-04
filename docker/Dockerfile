FROM jellyfin/jellyfin:10.11.6 AS jf

# ── Recipe planner (Bookworm is fine, no FFmpeg needed here) ─────────────────
FROM lukemathwalker/cargo-chef:latest-rust-slim-bookworm AS planner
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Builder: Debian Trixie ────────────────────────────────────────────────────
FROM debian:trixie-slim AS builder
ENV DEBIAN_FRONTEND=noninteractive
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        curl ca-certificates \
        clang pkg-config build-essential libssl-dev \
        libgstreamer1.0-dev \
        libgstreamer-plugins-base1.0-dev \
        libgstreamer-plugins-bad1.0-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"

RUN cargo install cargo-chef --locked \
    && rustup target add wasm32-unknown-unknown \
    && cargo install dioxus-cli --locked

COPY vendor ./vendor
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cd crates/dashboard && dx build --release

ENV SQLX_OFFLINE=true
RUN cargo build --release -p remux-server && strip target/release/remux-server

# ── Runtime ──────────────────────────────────────────────────────────────────
FROM debian:trixie-slim AS release
ENV DEBIAN_FRONTEND=noninteractive
ENV WEB_PATH=/app/jellyfin-web
ENV DASHBOARD_PATH=/app/dashboard

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        libgstreamer1.0-0 \
        gstreamer1.0-plugins-base \
        gstreamer1.0-plugins-good \
        gstreamer1.0-plugins-bad \
        gstreamer1.0-plugins-ugly \
        gstreamer1.0-libav \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/remux-server .
COPY --from=builder /app/target/dx/dashboard/release/web/public /app/dashboard
COPY --from=jf /jellyfin/jellyfin-web ${WEB_PATH}

EXPOSE 3000
CMD ["./remux-server"]
