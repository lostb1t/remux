## A Jellyfin server replacement based on stremio addons


Total replacement for the Jellyfin server.
Instead of local files, everything is routed to stremio addons.
Aims to be compatible with all jellyfin clients (eventually)

Status: Very experimental 🔥 it's a proof of concept

Highly recommend using https://github.com/Viren070/AIOStreams to manage your addons.

create a config.toml in the /data dir

```toml
[[users]]
key = "test_user"
username = "test"
password = "test"
aio_url = "https://myaiostreams"
```

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

### What works

- libraries
- collections (mapped to catalogs)
- search
- home view with builtin rows: recently added movies and recently added shows
- direct playback
- stream selection

### Wat does not work yet (todo)

- only tested with streamyfin so use that
- shows
- p2p
- favorites
- external subtitles
- transcoding
- trailers
- anything persistant like continue watching etc
- no user management yet, so auth is open. Can use any user/pw combi
- filtering does not work in library view and probaply never will