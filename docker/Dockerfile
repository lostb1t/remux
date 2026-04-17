FROM debian:trixie-slim

ARG TARGETARCH

ENV DEBIAN_FRONTEND=noninteractive
ENV WEB_PATH=/app/jellyfin-web
ENV ANFITEATRO_WEB_PATH=/app/anfiteatro-web
ENV DASHBOARD_PATH=/app/dashboard
ENV DATABASE_URL=sqlite:///data/db.sqlite?mode=rwc
ENV LOG_FILE=/data/logs/remux.jsonl
ENV TORRENT_DATA_DIR=/data/torrents
ENV FFMPEG_PATH=/app/jellyfin-ffmpeg/ffmpeg
ENV FFPROBE_PATH=/app/jellyfin-ffmpeg/ffprobe

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && YTDLP_ARCH=$([ "$TARGETARCH" = "arm64" ] && echo "yt-dlp_linux_aarch64" || echo "yt-dlp_linux") \
    && curl -L "https://github.com/yt-dlp/yt-dlp/releases/latest/download/${YTDLP_ARCH}" -o /usr/local/bin/yt-dlp \
    && chmod +x /usr/local/bin/yt-dlp \
    && apt-get purge -y --auto-remove curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY remux-server .
COPY dashboard ./dashboard
COPY jellyfin-web ./jellyfin-web
COPY anfiteatro-web ./anfiteatro-web
COPY jellyfin-ffmpeg ./jellyfin-ffmpeg

EXPOSE 3000
CMD ["./remux-server"]
