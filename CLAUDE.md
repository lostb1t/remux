# remux-server

## What this project is

A **Jellyfin-compatible media server** written in Rust. The HTTP API is a drop-in
replacement for the Jellyfin API — standard Jellyfin clients (web, mobile, desktop)
connect to it without modification.

## API design — critical rules

The server is a **Jellyfin API compatibility layer**. Every endpoint must match the
Jellyfin API contract exactly: same paths, same HTTP methods, same JSON field names
(PascalCase via `#[serde(rename_all = "PascalCase")]`), same status codes.

When adding or modifying an endpoint:
- Look up the Jellyfin OpenAPI spec or source to confirm the expected request/response
  shape before writing any code.
- Never rename fields, change casing, or alter status codes for style reasons.
- Remux-specific additions are fine — extra fields on existing DTOs, or new endpoints
  under non-Jellyfin paths — but must not break clients that ignore unknown fields.
- Remux-specific fields on shared DTOs are marked with a `// Remux:` comment.

## Crates

- `crates/server` — remux-server binary (Axum + SQLite via sqlx). Routes auto-registered via `inventory`.
- `crates/remux-macros` — proc-macros: `#[get]`, `#[post]`, `#[route]` for auto-route registration.
- `crates/shared` — Jellyfin DTOs + SDK clients. **Canonical source for all DTO types.** Used by server and dashboard.
- `crates/dashboard` — Dioxus WASM admin app served at `/admin`.

## Key files

| File | Purpose |
|---|---|
| `crates/server/jellyfin/api/system.rs` | System / branding endpoints |
| `crates/server/jellyfin/api/users.rs` | User endpoints |
| `crates/server/jellyfin/models.rs` | Re-exports shared DTOs + free conversion fns |
| `crates/shared/src/sdks/jellyfin/models.rs` | All Jellyfin DTO types (canonical) |
| `crates/server/db/user.rs` | `User` struct, auth, argon2 password hashing |
| `crates/server/db/media.rs` | Media CRUD + play state |
| `crates/server/conversions.rs` | `From` impls + free conversion fns for local types |
| `crates/server/web_patches.rs` | `CSS` / `JS` constants — edit here to patch jellyfin-web |

## Orphan rule: db → Jellyfin DTO conversions

Jellyfin DTOs live in `shared`, so `impl From<db::X> for jellyfin::Y` violates E0117.
All such conversions are free functions in `crates/server/jellyfin/models.rs`:
`db_user_to_dto`, `db_media_to_item`, `db_state_to_dto`, etc.

## Error helpers

Import `axum_anyhow::IntoApiError`, then use on any `Result`:
- `.context_bad_request(title, detail)` → 400
- `.context_unauthorized(title, detail)` → 401
- `.context_forbidden(title, detail)` → 403
- `.context_not_found(title, detail)` → 404

## Testing

Helpers live in `crates/server/integration_test.rs` (declared in `main.rs` under `#[cfg(test)]`).

### Server setup

```rust
let server = new_test_server().await.unwrap();       // unauthenticated
let (server, token) = authenticated_server().await;  // admin "test"/"test" already signed in
let auth = auth_header_with_token(&token);
```

### JSON assertions — always use axum-test helpers, never manual field indexing

```rust
// Exact shape. Use expect_json matchers for dynamic values:
resp.assert_json(&json!({
    "Id": expect_json::uuid(),
    "Name": "Remux",
}));

// Partial match — only checks listed fields, ignores everything else.
// Use when the response has many fields and you only care about a subset:
resp.assert_json_contains(&json!({
    "ServerName": "Remux",
}));
```

Rules:
- Always prefer `assert_json` / `assert_json_contains` over `body["Field"] == ...`.
- Use `expect_json::uuid()` for UUID fields instead of pre-extracting them.
- Use `expect_json::iso_date_time()` for timestamps.
- Use `assert_json_contains` for large responses where only a few fields matter
  (e.g. `ServerConfiguration` has 16+ fields).
- Use `assert_json` (exact) when the full shape is small and fully known.

### Auth pattern

```rust
// Protected endpoint — happy path:
let (server, token) = authenticated_server().await;
server
    .get("/protected")
    .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth_header_with_token(&token)).unwrap())
    .await
    .assert_status_ok();

// Auth guard check:
new_test_server().await.unwrap()
    .get("/protected")
    .expect_failure()
    .await
    .assert_status(StatusCode::UNAUTHORIZED);
```

### Conversions

When casting from and to different structs and enums. Try to use the from or tryfrom trait as much as possible.