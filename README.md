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

Stream content from Stremio add-ons, local files, or WebDAV sources all through your existing Jellyfin clients.

Movies and shows come from Stremio add-ons or your own files. Music is handled separately through its own streaming pipeline.

Use your existing Jellyfin clients as-is: browse, search, and play.

Built in Rust for performance and low resource usage.

---

## What makes it different from Jellyfin?

- **Online sources or local files**  
  Stream from Stremio add-ons, a local path, or a WebDAV server

- **Stremio-powered video**  
  Movies and shows come from Stremio add-ons

- **Independent music pipeline**  
  Music is not tied to Stremio and is streamed from separate sources

- **IPTV Support**  
  
- **Dynamic libraries**  
  Build collections based on filters instead of folders or scans

- **User management**
  Including user data import from jellyfin servers to get you started
  
- **Lightweight & fast**  
  Written in Rust with a focus on efficiency

- **New dashboard**  
  A custom-built admin interface tailored for this workflow

- **No backend plugins**  
  Simpler architecture (Jellyfin web UI theming still works)

- **Local & WebDAV files**  
  Index and stream video, audio, or `.strm` files from a local path or WebDAV server


## ⚠️ Status

Remux is still in an early stage. Expect rough edges, missing features, and breaking changes.
Run the image as follows

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

Build jellyfin web

```
cargo make jellyfin-web
```

Fetch/build all supported web clients (Jellyfin)

```
cargo make web-clients
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
> **AI Policy:** Use AI as much as you want, but understand every line, verify it works, communicate as a human, and disclose significant AI-generated contributions.

We welcome contributions created with the help of AI tools such as GitHub Copilot, Claude, ChatGPT, Cursor, and similar assistants. AI is a tool; contributors remain responsible for everything they submit.  

1. AI-assisted code is allowed

You may use AI to:

* Generate code
* Draft tests
* Research the codebase
* Suggest fixes and improvements
* Help write documentation

All contributions must still meet the project’s quality standards and pass review.  

2. You are responsible for your contributions

Before submitting a pull request, you must:

* Understand the code you are submitting
* Be able to explain why it works
* Test your changes
* Review and edit any AI-generated content

Do not submit code you do not understand.  

3. Communication must be human

When interacting with maintainers and reviewers:

* Write your own PR descriptions
* Write your own review responses
* Be prepared to discuss your changes

AI may help you draft a response, but maintainers expect to communicate with the contributor, not an AI assistant.  

4. Disclose significant AI usage

If AI generated a substantial portion of your code, documentation, issue, or pull request, mention it in the PR description.

Example:

Initial implementation drafted with Claude and reviewed, tested, and modified by me.

This disclosure helps reviewers understand how the contribution was created.  

⸻



Use AI as much as you want, but understand every line, verify it works, communicate as a human, and disclose significant AI-generated contributions.

That single sentence captures about 90% of the original policy.
