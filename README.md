## Remux Server

Remux is a self-hosted media server that exposes a Jellyfin-compatible API while sourcing media from Stremio add-ons instead of a local file library. The goal is to let existing Jellyfin clients browse catalogs, search content, choose streams, and play media through Remux without client-side changes.

The project is built in Rust and currently targets a lightweight, add-on-backed streaming workflow. It is still experimental and should be treated as a proof of concept.

Only AIOStreams is supported for now: https://github.com/Viren070/AIOStreams.

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

Fetch/build all supported web clients (Jellyfin + Anfiteatro)

```
cargo make web-clients
```

run

```
cargo make dev
```
