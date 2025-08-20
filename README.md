Use stremio addons with Jellyfin clients.

Total replacement for the Jellyfin server.  
Instead of local files, everything is routed to stremio addons.  
Aims to be compatible with all jellyfin clients (eventually)

Status: Very experimental 🔥

Highly recommend using https://github.com/Viren070/AIOStreams to manage your addons.

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

- libraries
- collections (mapped to catalogs)
- search
- home view with builtin rows: recently added movies and recently added shows
- direct playback
- stream selection

### Wat does not work yet

- only tested with streamyfin so use that
- shows
- p2p
- favorites
- external subtitles
- anything persistant like continue watching etc
- no user management yet, so auth is open. Can use any user/pw combi
- filtering does not work in library view and probaply never will