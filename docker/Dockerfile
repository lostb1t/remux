FROM jellyfin/jellyfin:10.11.6 AS jf

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
COPY remux-server .
COPY dashboard ./dashboard
COPY --from=jf /jellyfin/jellyfin-web ./jellyfin-web

EXPOSE 3000
CMD ["./remux-server"]
