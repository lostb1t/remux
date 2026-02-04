## A Jellyfin server replacement based on stremio addons


Total replacement for the Jellyfin server.
Instead of local files, everything is routed to stremio addons.
Aims to be compatible with all jellyfin clients (eventually)

Status: Very experimental 🔥 it's a proof of concept

Only supports aiostreams for now https://github.com/Viren070/AIOStreams.

create a config.toml in the /data dir

```toml
aio_url = "https://<your_aiostreams_manifest_url>"

[[users]]
key = "test_user"
username = "test"
password = "test"
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

setup db

```
cargo make resetdb
```

run

```
cargo make dev
```