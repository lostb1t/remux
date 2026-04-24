# Remux

Remux is a self-hosted media server with a Jellyfin-compatible API that streams content from online sources instead of a local library.

Movies and shows come from Stremio add-ons. Music is handled separately through its own streaming pipeline.

Use your existing Jellyfin clients as-is: browse, search, and play. No library scans, no file management, no traditional backend.

Built in Rust for performance and low resource usage.

---

## What makes it different from Jellyfin?

- **No local library required**  
  Content is streamed from online sources instead of files on disk

- **Stremio-powered video**  
  Movies and shows come from Stremio add-ons

- **Independent music pipeline**  
  Music is not tied to Stremio and is streamed from separate sources

- **Dynamic libraries**  
  Build collections based on filters instead of folders or scans

- **Lightweight & fast**  
  Written in Rust with a focus on efficiency

- **New dashboard**  
  A custom-built admin interface tailored for this workflow

- **No backend plugins**  
  Simpler architecture (Jellyfin web UI theming still works)

- **Local files (indirectly)**  
  Possible via Stremio add-ons, but not natively supported


## ⚠️ Status

Remux is still in an early stage. Expect rough edges, missing features, and breaking changes.
Run the image as follows

```yml
version: "3"
services:
  remux:
    image: ghcr.io/lostb1t/remux:nightly
    ports:
      - "3000:3000"
    volumes:
      /remux/data:/data
```

### Development

Install cargo make

```
cargo install --force cargo-make
```

Build jellyfin web

```
cargo make jellyfin-web
```

Fetch/build all supported web clients (Jellyfin + Anfiteatro)

```
cargo make web-clients
```

run

```
cargo make dev
```
