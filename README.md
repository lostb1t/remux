
- only tested with streamyfin so use that
- only direct play currently supported
- shows currently do not work.
- p2p does not work yet
- favorites does not work yet
- no user management yet, so auth is open. Can use any user/pw combi
- filtering does not work in librery view and probaply never will

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