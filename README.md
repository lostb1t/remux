## Remux

Remux is a self-hosted media server that exposes a Jellyfin-compatible API while sourcing media from Stremio add-ons instead of a local file library. The goal is to let existing Jellyfin clients browse catalogs, search content, choose streams, and play media through Remux without client-side changes.

The project is built in Rust and currently targets a lightweight, add-on-backed streaming workflow. It is still experimental and should be treated as a proof of concept.

Main difference compared to jellyfin

- Built in rust, faster and lower resource usage
- Complete new dashboard.
- Dynamic library and collection system. Create libraries from a custom set of filters. No foldersmor scans.
- Stream music from online sources
- No backend plugins (jelkyfin web ui theming does work so tou can still use your favorite theme)
- No native local file support tho local filea can be used through stremio addons

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
