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

---

Remux is a Jellyfin-compatible media server that brings Stremio add-ons, local files, and WebDAV sources together under one roof. Music streams from its own dedicated pipeline with support for remote sources. Use any Jellyfin client to browse, search, and play without any client changes. Written in Rust.

---

## Credits & Attribution

This repository is a **custom fork of [Remux](https://github.com/remux-org/remux)**, the original open-source Jellyfin-compatible media server project. The upstream project remains the primary source for the Remux server architecture, Jellyfin API compatibility, and the original project history.

This fork focuses on an integrated AIO deployment profile with automatic, pre-configured support for [AIOStreams](https://github.com/Viren070/AIOStreams) and [AIOMetadata](https://github.com/cedya77/aiometadata). Its goal is to provide a ready-to-run bridge between the AIO ecosystem and official Jellyfin-compatible clients such as Infuse and VidHub, while preserving the upstream Remux behavior and compatibility wherever possible.

All upstream copyright notices, attribution requirements, and license terms remain applicable. This project **retains the original Remux license**; see the [`LICENSE`](LICENSE) file for the complete terms. Changes and additions specific to this fork are documented in the repository history.

---

## Features

- **Works with your Jellyfin clients**  
  Infuse, Swiftfin, Jellyfin for Android, and any other Jellyfin-compatible client works without changes.

- **Multiple content sources**  
  Stream from Stremio add-ons, local files, WebDAV servers, or torrents. Mix and match across a single library.

- **Built-in torrent streaming**  
  Stream directly from torrents without a separate client. No downloads required.

- **Independent music pipeline**  
  Music is not tied to Stremio and streams from its own sources, including remote ones.

- **Probe data for streams**  
  Audio and subtitle track selection works out of the box for streamed content. Track metadata is sourced from [RemuxDB](https://remuxdb.1632022.xyz) so clients see the same experience as local files.

- **Powerful library filtering**  
  Build libraries dynamically: filter by tags, catalogs, popularity, release year, and more. Exclude content per-user or scope libraries to specific audiences without duplicating sources.

- **Playback tracking**  
  Progress syncs across clients with continue watching support.

- **User management**  
  Import users and data from an existing Jellyfin server to get started quickly.

- **Desktop app**  
  Single install, no Docker or terminal required. The server runs in the background as a tray app.

- **Custom dashboard**  
  A built-in admin interface designed for this workflow.

- **IPTV Support**


## Quick Start

### Desktop

Download the latest release for your platform:

- [macOS (Apple Silicon)](https://github.com/lostb1t/remux/releases/latest/download/remux-desktop-macos-aarch64.dmg)
- [Linux (x86_64)](https://github.com/lostb1t/remux/releases/latest/download/remux-desktop-linux-x86_64.deb)
- [Windows (x86_64)](https://github.com/lostb1t/remux/releases/latest/download/remux-desktop-windows-x86_64.zip)

### Docker

```yml
version: "3"
services:
  remux:
    image: ghcr.io/lostb1t/remux:latest # or nightly
    ports:
      - "3000:3000"
    volumes:
      - /remux/data:/data
```

### Docker Compose with AIOStreams and AIOMetadata

The repository's `docker/compose.yml` starts Remux together with AIOStreams and AIOMetadata. Remux reaches both addons over the private `remux-internal` network, while only the Remux HTTP port is published on the host. The AIO containers also use a separate non-published egress network because they must contact their upstream metadata, catalog, debrid, and provider services.

Before starting the stack, create a local environment file and never commit it:

```sh
cp .env.example .env
```

At minimum, AIOStreams requires a persistent `SECRET_KEY`. Add a securely generated value to `.env`, together with any provider credentials required by the AIOStreams configuration you use. For example:

```dotenv
# Generate a strong value; do not use this example literally.
SECRET_KEY=replace-with-a-64-character-hex-secret

# Add the provider variables required by your AIOStreams release/configuration.
# Keep Debrid tokens and API keys in .env or in the AIOStreams web configurator,
# never in docker/compose.yml or in Git.
# REAL_DEBRID_API_KEY=...
# TORBOX_API_KEY=...
```

The Compose file passes `.env` to both AIO containers and persists their `/app/data` directories, so configured provider credentials and addon state survive container recreation:

```yaml
services:
  aiostreams:
    env_file: .env
    volumes:
      - ./data/aiostreams:/app/data
  aiometadata:
    env_file: .env
    volumes:
      - ./data/aiometadata:/app/data
```

The checked-in Compose file already defines the two internal manifest URLs used by Remux:

```dotenv
AIO_STREAMS_MANIFEST_URL=http://aiostreams:3000/manifest.json
AIO_METADATA_MANIFEST_URL=http://aiometadata:3000/manifest.json
```

If a provider requires additional environment variables, add them to the local `.env` according to the [AIOStreams configuration reference](https://docs.aiostreams.viren070.me/) and the corresponding AIOMetadata release documentation. Then start the stack with:

```sh
docker compose -f docker/compose.yml up -d
```

For production, pin image versions instead of `latest`, restrict `.env` permissions (`chmod 600 .env`), and use a secret manager where available. Do not publish the AIO service ports directly; Remux should access them by the internal container names `aiostreams` and `aiometadata`.

### Development

Install cargo make

```
cargo install --force cargo-make
```

Install the dioxus cli

```
cargo install dioxus-cli
```

Copy env example

```
cp .env.example .env
```

Build jellyfin web

```
cargo make jellyfin-web
```

run

```
cargo make dev
```

### ❤️ Support the Project

- ⭐ **[Star the repository](https://github.com/lostb1t/remux)** on GitHub.
- 🤝 **Contribute**: Report issues, suggest features, or submit pull requests.
- ☕ **Donate**:
  - **[Ko-fi](https://ko-fi.com/lostb1t)**

### AI policy

> [!IMPORTANT]
> Use AI as much as you want, but understand every line, verify it works, communicate as a human, and disclose significant AI-generated contributions.

We welcome contributions created with the help of AI tools such as GitHub Copilot, Claude, ChatGPT, Cursor, and similar assistants. AI is a tool; contributors remain responsible for everything they submit.  

#### AI-assisted code is allowed

You may use AI to:

* Generate code
* Draft tests
* Research the codebase
* Suggest fixes and improvements
* Help write documentation

All contributions must still meet the project’s quality standards and pass review.  

#### You are responsible for your contributions

Before submitting a pull request, you must:

* Understand the code you are submitting
* Be able to explain why it works
* Test your changes
* Review and edit any AI-generated content

Do not submit code you do not understand.  

#### Communication must be human

When interacting with maintainers and reviewers:

* Write your own PR descriptions
* Write your own review responses
* Be prepared to discuss your changes

AI may help you draft a response, but maintainers expect to communicate with the contributor, not an AI assistant.  

#### Disclose when AI was used

If AI was used to generate a significant portion of an issue, PR, or the code it contains, please say so in the submission. A short note in the PR description is enough — for example, "The initial implementation was drafted with Claude and then reviewed and edited by me."

Issues and pull requests that appear to be AI-generated but do not disclose it may be closed without review. Contributors who repeatedly submit undisclosed AI content, or who ignore this policy, may be blocked from contributing.

### Keep It Human

We're grateful for all genuine contributions, whether AI-assisted or not. The key is human oversight and understanding. Thank you for helping keep Actual focused on what matter
