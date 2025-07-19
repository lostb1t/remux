FROM rust:latest AS builder
WORKDIR /app

RUN cargo install dioxus-cli@0.7.0-alpha.2

COPY . .
RUN dx bundle --release --platform web

#from debian:bookworm-slim as release
FROM ghcr.io/static-web-server/static-web-server:2-debian

COPY --from=builder /app/target/dx/remux/release/web /public

WORKDIR /app
ENV PORT=80
ENV IP=0.0.0.0

#CMD ["./remux"]
#CMD ["dx", "serve", "--release"]