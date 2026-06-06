# remux-server

Jellyfin-compatible media server written in Rust.

## Crates

| Crate | Purpose |
|---|---|
| `remux-server` | Core library and server binary. Contains all API handlers, DB layer, services, transcoding, and the axum router. |
| `remux-desktop` | macOS/Windows system-tray app. Embeds the server and launches it in a background thread, opens a browser window to the admin UI. |
| `remux-dashboard` | Dioxus (WASM) frontend for the admin dashboard, served by the server at `/admin`. |
| `remux-macros` | Proc-macros for axum route auto-registration (`#[get]`, `#[post]`, etc.) via `inventory`. |
| `remux-sdks` | Typed API clients for third-party services: Stremio, IntroDB, and a partial TMDB client. |
| `remux-utils` | Shared utilities: an in-memory `Store` (LRU cache), the `retry!` macro, and UUID helpers. |

## Architecture

- **State**: `AppState` → `AppContext` → `Config`. Handlers receive `State(state): State<AppState>` and access config via `state.ctx.config`.
- **Database**: SQLite via SQLx. Connection string lives in `Config.database_url`.

In general use Parse, don't Validate and Type-Driven Design in Rust.

## Configuration

All operator-configurable settings must be fields on `Config` (in `crates/remux-server/src/lib.rs`), loaded via the `config` crate from a config file and environment variables. Do **not** read runtime settings with bare `std::env::var` — add a field to `Config` instead so it participates in the full config system (file, env var, serde default, serialize for logging).

Exceptions: bootstrapping env vars that must be read before `Config` is loaded (`CONFIG`, `FFMPEG_PATH`, `FFPROBE_PATH`) stay as direct `std::env::var` reads.

If a field's default depends on another field (e.g. `database_url` derived from `data_dir`), use `Option<String>` for the derived field and implement `Config::resolve()` to fill it in post-deserialization. Call `.resolve()` in `main.rs` immediately after loading.

## Coding conventions

- Prefer `strum`-derived enums over raw strings for any value that has a fixed set of variants (media kinds, image types, codec names, etc.). Use `#[derive(EnumString, Display, ...)]` so the enum round-trips cleanly through serde and DB layers without stringly-typed branches.


## Policy filter rules

Filter rules must **not** apply to collection/folder container queries — only to content items. See `get_by_filter` in the db layer.

## API conventions

- API handler paths must always be lowercase (e.g. `#[get("/useritems/{id}")]`, not `/UserItems/{Id}`).
- The API MUST remain fully compatible with Jellyfin’s public API shape and behavior.
- All endpoints in `remux-server/src/api` should mimic Jellyfin’s API exactly in:
  - route paths
  - request/response structure
  - field names
  - field semantics

### Extension rules (remux namespace)

- Custom fields are allowed only as **additive extensions**.
- All custom fields MUST live under a `remux` namespace object.
  Example:
  ```json
  {
    "Id": "abc",
    "Name": "Movie",
    "remux": {
      "custom_field": "value"
    }
  }