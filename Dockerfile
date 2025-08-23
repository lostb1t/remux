FROM jellyfin/jellyfin:10.10.7 AS web

FROM rust:slim-bookworm AS builder
WORKDIR /app

RUN apt update \
    && apt install -y ffmpeg clang pkg-config libavutil-dev libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev libswresample-dev libswscale-dev \
    && apt clean \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release

RUN rm -rf src
COPY ./src ./src
RUN cargo build --release

RUN strip target/release/remux-server

from debian:bookworm-slim as release
ENV WEB_PATH =/app/jellyfin-web

RUN apt update \
    && apt install -y ffmpeg \
    && apt clean \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

WORKDIR /app
COPY --from=builder /app/target/release/remux-server .
COPY --from=web /jellyfin/jellyfin-web /app/jellyfin-web

CMD ["./remux-server"]