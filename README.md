## A Jellyfin server replacement based on stremio addons


Total replacement for the Jellyfin server.
Instead of local files, everything is routed to stremio addons.
Aims to be compatible with all jellyfin clients (eventually)

Status: Very experimental 🔥 it's a proof of concept

Only supports aiostreams for now https://github.com/Viren070/AIOStreams.

A self-hosted, Jellyfin-compatible media server that acts as a full replacement for Jellyfin.
It integrates Stremio add-ons directly into the Jellyfin ecosystem by exposing a fully compatible Jellyfin API layer, allowing existing Jellyfin clients to work without modification.
Built in Rust for high performance, low resource usage, and an optimized streaming experience.

Run the image as follows

```yml
version: "3"
services:
  remux:
    image: ghcr.io/lostb1t/remux-server
    ports:
      - "3000:3000"
    volumes:
      /remux/data:/data
```

Open up it up in your browser and start importing catalogs byt creating
new collections backed by aio catalogs.

### Whats included

- libraries
- collections (mapped to catalogs)
- search
- home view with builtin rows: recently added movies and recently added shows
- direct playback
- stream selection
- user/auth

### Whats missing

- admin
- favorites
- external subtitles
- transcoding
- trailers
- anything persistant like continue watching etc

### Development

Install cargo make

```
cargo install --force cargo-make
```

Build jellyfin web

```
cargo make jellyfin-web
```

run

```
cargo make dev
```