<div align="center">
   <img width="200" height="200" src="logo.png" alt="Logo">
</div>

<div align="center">
  <h1><b>Remux</b></h1>
  <p><i>self-hosted media server with a Jellyfin-compatible API</i></p>
<a href="https://discord.gg/rEbhk4RBhs">
    <img src="https://img.shields.io/badge/Talk%20on-Discord-brightgreen">
</a>
</div>


Stream content from Stremio add-ons, local files, or WebDAV sources all through your existing Jellyfin clients.

Movies and shows come from Stremio add-ons or your own files. Music is handled separately through its own streaming pipeline.

Use your existing Jellyfin clients as-is: browse, search, and play. No library scans, no file management, no traditional backend.

Built in Rust for performance and low resource usage.



---

## What makes it different from Jellyfin?

- **Online sources or local files**  
  Stream from Stremio add-ons, a local path, or a WebDAV server

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

- **Local & WebDAV files**  
  Index and stream video, audio, or `.strm` files from a local path or WebDAV server


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
