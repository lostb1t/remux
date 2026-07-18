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

## Admin dashboard conventions (`remux-dashboard`)

- **Theme presets are paired with CSS.** Every entry in `THEME_PRESETS`
  (`src/theme.rs`) except `default` MUST have a matching
  `:root[data-preset="<id>"]` block in `assets/theme.css`, or selecting it
  applies no palette. This is enforced by the `every_preset_has_a_css_block`
  test — run `cargo test -p remux-dashboard` after touching either.
- **New pages** = a `Route` variant (`src/router.rs`) + a `NavSubItem`
  (`src/layout.rs`, plus `page_title`/breadcrumb `section` arms) + a page under
  `src/pages/` + a typed `Endpoint` command in `remux-sdks`. Extract pure
  helpers (formatting, mapping) as free functions with colocated `#[cfg(test)]`
  tests — the dashboard is a `bin` crate, so run tests with
  `cargo test -p remux-dashboard` (no `--lib`).

## Activity log

The audit log (`GET /System/ActivityLog/Entries`) is backed by the
`activity_log` table via `db::ActivityLog`. Record real events at their source
with `db::ActivityLog::record_ignore(...)` (fire-and-forget; never fails the
request). Use the `ActivityKind`/`ActivitySeverity` enums, not raw strings.
  Example:
  ```json
  {
    "Id": "abc",
    "Name": "Movie",
    "remux": {
      "custom_field": "value"
    }
  }

### Testing Guidelines

Unit Tests

Prefer placing unit tests in the same source file as the code being tested.

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn example() {
        // ...
    }
}

Reasons:

* Keeps tests close to the implementation.
* Makes it easier to maintain tests when code changes.
* Allows testing of private functions, structs, and modules.
* Follows common Rust ecosystem conventions.

Unit tests should be used for:

* Helper functions
* Parsing logic
* Business rules
* Data transformations
* Utility modules
* Small isolated components

Integration Tests

Place integration tests in the tests/ directory.

tests/
├── api.rs
├── database.rs
└── workflows.rs

Integration tests should only interact with the crate through its public API.

Use integration tests for:

* End-to-end workflows
* Public API validation
* Database interactions
* HTTP endpoints
* Cross-module behavior
* Regression tests spanning multiple components

Preferred Approach

Default to colocated unit tests (#[cfg(test)]) unless the test exercises behavior across multiple modules or validates the public API. Small focused tests should remain alongside the implementation.

When adding new functionality, prefer adding tests to the same file rather than creating a new integration test unless there is a clear end-to-end testing requirement.