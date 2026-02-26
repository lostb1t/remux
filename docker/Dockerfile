FROM jellyfin/jellyfin:10.11.5 AS jf

FROM rust:slim-bookworm AS builder
WORKDIR /app

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

COPY vendor ./vendor
COPY Cargo.toml Cargo.lock ./
COPY crates/remux-macros ./crates/remux-macros
COPY crates/server/Cargo.toml ./crates/server/Cargo.toml
COPY crates/dashboard ./crates/dashboard
RUN cargo fetch

COPY . .

# Build dashboard WASM (outputs to crates/dashboard/dist/)
RUN cd crates/dashboard && dx build --release

ENV SQLX_OFFLINE=true
RUN cargo build --release

FROM debian:bookworm-slim AS release
ENV WEB_PATH=/app/jellyfin-web

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
