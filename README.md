## Remux

Remux is a self-hosted media server that exposes a Jellyfin-compatible API while sourcing media from Stremio add-ons instead of a local file library. The goal is to let existing Jellyfin clients browse catalogs, search content, choose streams, and play media through Remux without client-side changes.

The project is built in Rust and currently targets a lightweight, add-on-backed streaming workflow. It is still experimental and should be treated as a proof of concept.

Run the image as follows

```yml
version: "3"
services:
  remux:
    image: ghcr.io/lostb1t/remux
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
