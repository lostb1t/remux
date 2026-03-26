FROM jellyfin/jellyfin:10.11.6 AS jf

# ── Recipe planner ────────────────────────────────────────────────────────────
FROM lukemathwalker/cargo-chef:latest-rust-slim-bookworm AS planner
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json -p remux-server

# ── Builder: Debian Trixie ────────────────────────────────────────────────────
FROM debian:trixie-slim AS builder
ENV DEBIAN_FRONTEND=noninteractive
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        curl ca-certificates \
        clang pkg-config build-essential libssl-dev \
        libavcodec-dev libavformat-dev libavfilter-dev \
        libavutil-dev libavdevice-dev libswscale-dev libswresample-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"

RUN cargo install cargo-chef --locked

COPY vendor ./vendor
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json -p remux-server

COPY . .

ENV SQLX_OFFLINE=true
RUN cargo build --release -p remux-server && strip target/release/remux-server

# ── Runtime ──────────────────────────────────────────────────────────────────
FROM debian:trixie-slim AS release
ENV DEBIAN_FRONTEND=noninteractive
ENV WEB_PATH=/app/jellyfin-web
ENV DASHBOARD_PATH=/app/dashboard
ENV DATABASE_URL=sqlite:///data/db.sqlite?mode=rwc
ENV LOG_FILE=/data/logs/remux.jsonl
ENV TORRENT_DATA_DIR=/data/torrents

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ffmpeg \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/remux-server .
COPY --from=builder /app/target/dx/remux-dashboard/release/web/public /app/dashboard
COPY --from=jf /jellyfin/jellyfin-web /app/jellyfin-web

EXPOSE 3000
CMD ["./remux-server"]
