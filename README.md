Total replacement for the Jellyfin server.

Instead of local files, everything is routed to stremio addons.

Aims to be compatible with all jellyfin clients (eventually)

Very experimental 🔥

```yml
version: "3"
services:
  remux-server:
    image: ghcr.io/lostb1t/remux-server:latest
    environment:
      ADDONS: '["https://torrentio.strem.fun/manifest.json","https://v3-cinemeta.strem.io/manifest.json"]'
    ports:
      - "3000:3000"
```

### What works

- library view
- Collections (mapped to catalogs)
- search
- builtin rows: recently added movies and recently added shows
- direct playback

### Wat does not work yet

- only tested with streamyfin so use that
- shows currently do not work.
- p2p does not work yet
- favorites does not work yet
- no user management yet, so auth is open. Can use any user/pw combi
- filtering does not work in library view and probaply never will