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
      /remux/data:/data
```

### Development

Install cargo make

```
cargo install --force cargo-make
```

Install the dioxus cli

```
cargo install dioxus-cli
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
