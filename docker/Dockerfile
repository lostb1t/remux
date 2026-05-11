FROM debian:trixie-slim

ENV DEBIAN_FRONTEND=noninteractive
ENV WEB_PATH=/app/jellyfin-web
ENV DASHBOARD_PATH=/app/dashboard
ENV DATABASE_URL=sqlite:///data/db.sqlite?mode=rwc
ENV LOG_FILE=/data/logs/remux.jsonl
ENV TORRENT_DATA_DIR=/data/torrents
ENV FFMPEG_PATH=/app/jellyfin-ffmpeg/ffmpeg
ENV FFPROBE_PATH=/app/jellyfin-ffmpeg/ffprobe
ENV LIBVA_DRIVERS_PATH=/usr/lib/x86_64-linux-gnu/dri:/app/jellyfin-ffmpeg/lib/dri
ENV PATH="/app/jellyfin-ffmpeg:${PATH}"
ENV NVIDIA_VISIBLE_DEVICES=all
ENV NVIDIA_DRIVER_CAPABILITIES=compute,video,utility

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        python3 \
        python3-pip \
        libva2 \
        libva-drm2 \
        libdrm2 \
        mesa-va-drivers \
        vainfo \
        libfontconfig1 \
        libfreetype6 \
        libssl3 \
    && pip3 install --break-system-packages yt-dlp "curl-cffi>=0.10,<0.15" \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY remux-server .
COPY dashboard ./dashboard
COPY jellyfin-web ./jellyfin-web
COPY jellyfin-ffmpeg ./jellyfin-ffmpeg

EXPOSE 3000

CMD ["./remux-server"]