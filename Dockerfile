FROM rust:latest AS builder
WORKDIR /app

RUN cargo install dioxus-cli@0.7.0-alpha.2

COPY . .
RUN dx bundle --release --platform web

from debian:bookworm-slim as release
#RUN apt update \
#    && apt install -y openssl ca-certificates \
#    && apt clean \
#    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

COPY --from=builder /app/target/dx/remux/release/web /app

WORKDIR /app
ENV PORT=80
ENV IP=0.0.0.0

CMD ["./remux"]
#CMD ["dx", "serve", "--release"]