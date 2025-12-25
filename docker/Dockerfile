FROM jellyfin/jellyfin:10.10.7 AS web

FROM rust:slim-bookworm AS builder
WORKDIR /app

RUN apt update \
    && apt install -y curl clang pkg-config libavutil-dev libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev libswresample-dev libswscale-dev \
    && apt clean \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

ARG JF_FFMPEG_VERSION=7.1.1-7
RUN set -eux; \
    . /etc/os-release; \
    ARCH="$(dpkg --print-architecture)"; \
    CODENAME="${VERSION_CODENAME}"; \
    BASE_REPO="https://repo.jellyfin.org/master/files/ffmpeg/debian/7.x/${JF_FFMPEG_VERSION}/${ARCH}"; \
    FILE_NAME="jellyfin-ffmpeg7_${JF_FFMPEG_VERSION}-${CODENAME}_${ARCH}.deb"; \
    DEB_URL="${BASE_REPO}/${FILE_NAME}"; \
    echo "Downloading: ${DEB_URL}"; \
    curl -fL --retry 3 -o /tmp/jellyfin-ffmpeg7.deb "${DEB_URL}"; \
    apt-get update; \
    apt-get install -y --no-install-recommends /tmp/jellyfin-ffmpeg7.deb; \
    ln -sf /usr/lib/jellyfin-ffmpeg/ffmpeg  /usr/local/bin/ffmpeg; \
    ln -sf /usr/lib/jellyfin-ffmpeg/ffprobe /usr/local/bin/ffprobe; \
    ffmpeg -version; \
    rm -f /tmp/jellyfin-ffmpeg7.deb

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release

RUN rm -rf src
COPY ./src ./src
RUN cargo build --release

RUN strip target/release/remux-server

from debian:bookworm-slim as release
ENV WEB_PATH=/app/jellyfin-web

RUN apt update \
    && apt install -y ffmpeg \
    && apt clean \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

WORKDIR /app
COPY --from=builder /usr/local/bin/ffprobe /usr/local/bin/ffprobe
COPY --from=builder /usr/local/bin/ffmpeg /usr/local/bin/ffmpeg
COPY --from=builder /app/target/release/remux-server .
COPY --from=web /jellyfin/jellyfin-web /app/jellyfin-web

CMD ["./remux-server"]