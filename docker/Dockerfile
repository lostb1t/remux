FROM debian:trixie-slim AS ffmpeg-downloader

ARG TARGETARCH
ARG FFMPEG_VERSION=7.1.3-3

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN FFMPEG_ARCH=$([ "$TARGETARCH" = "amd64" ] && echo "linux64" || echo "linuxarm64") \
    && curl -fsSL "https://github.com/jellyfin/jellyfin-ffmpeg/releases/download/v${FFMPEG_VERSION}/jellyfin-ffmpeg_${FFMPEG_VERSION}_portable_${FFMPEG_ARCH}-gpl.tar.xz" \
         -o /tmp/ffmpeg.tar.xz \
    && mkdir -p /opt/jellyfin-ffmpeg \
    && tar -xJf /tmp/ffmpeg.tar.xz -C /opt/jellyfin-ffmpeg \
    && rm /tmp/ffmpeg.tar.xz

FROM debian:trixie-slim

ENV DEBIAN_FRONTEND=noninteractive
ENV WEB_PATH=/app/jellyfin-web
ENV DASHBOARD_PATH=/app/dashboard
ENV DATABASE_URL=sqlite:///data/db.sqlite?mode=rwc
ENV LOG_FILE=/data/logs/remux.jsonl
ENV TORRENT_DATA_DIR=/data/torrents
ENV FFMPEG_PATH=/app/jellyfin-ffmpeg/ffmpeg
ENV FFPROBE_PATH=/app/jellyfin-ffmpeg/ffprobe

ARG TARGETARCH

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=ffmpeg-downloader /opt/jellyfin-ffmpeg ./jellyfin-ffmpeg
COPY server-${TARGETARCH}/remux-server ./remux-server
RUN chmod +x ./remux-server ./jellyfin-ffmpeg/ffmpeg ./jellyfin-ffmpeg/ffprobe

COPY dashboard ./dashboard
COPY jellyfin-web ./jellyfin-web

EXPOSE 3000
CMD ["./remux-server"]
