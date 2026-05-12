FROM debian:trixie-slim

# https://github.com/intel/compute-runtime/releases
ARG GMMLIB_VER=22.8.1
ARG IGC2_VER=2.18.5
ARG IGC2_BUILD=19820
ARG NEO_VER=25.35.35096.9
ARG IGC1_LEGACY_VER=1.0.17537.24
ARG NEO_LEGACY_VER=24.35.30872.36

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
        curl \
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

# Intel OpenCL tone mapping (Gen12+ current + Gen11 legacy, amd64 only)
ARG TARGETARCH
RUN if [ "${TARGETARCH}" = "amd64" ]; then \
        mkdir intel-compute-runtime \
        && cd intel-compute-runtime \
        && apt-get update \
        && apt-get install --no-install-recommends --no-install-suggests -y ocl-icd-libopencl1 \
        && curl -LO https://github.com/intel/compute-runtime/releases/download/${NEO_VER}/libigdgmm12_${GMMLIB_VER}_amd64.deb \
                -LO https://github.com/intel/intel-graphics-compiler/releases/download/v${IGC2_VER}/intel-igc-core-2_${IGC2_VER}+${IGC2_BUILD}_amd64.deb \
                -LO https://github.com/intel/intel-graphics-compiler/releases/download/v${IGC2_VER}/intel-igc-opencl-2_${IGC2_VER}+${IGC2_BUILD}_amd64.deb \
                -LO https://github.com/intel/compute-runtime/releases/download/${NEO_VER}/intel-opencl-icd_${NEO_VER}-0_amd64.deb \
                -LO https://github.com/intel/intel-graphics-compiler/releases/download/igc-${IGC1_LEGACY_VER}/intel-igc-core_${IGC1_LEGACY_VER}_amd64.deb \
                -LO https://github.com/intel/intel-graphics-compiler/releases/download/igc-${IGC1_LEGACY_VER}/intel-igc-opencl_${IGC1_LEGACY_VER}_amd64.deb \
                -LO https://github.com/intel/compute-runtime/releases/download/${NEO_LEGACY_VER}/intel-opencl-icd-legacy1_${NEO_LEGACY_VER}_amd64.deb \
        && apt-get install --no-install-recommends --no-install-suggests -f -y ./*.deb \
        && cd .. \
        && rm -rf intel-compute-runtime \
    ; fi \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY remux-server .
COPY dashboard ./dashboard
COPY jellyfin-web ./jellyfin-web
COPY jellyfin-ffmpeg ./jellyfin-ffmpeg

EXPOSE 3000

CMD ["./remux-server"]