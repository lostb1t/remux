# Admin parity & jellyfin-web compatibility

Makes the bundled **jellyfin-web admin dashboard** work against Remux, and brings
the custom Dioxus **`/admin`** panel closer to parity — adding only pages backed
by real Remux config/data. Every change is additive and Jellyfin-shape-compatible.

## Why

Two problems:

1. **jellyfin-web's admin was unusable.** `web_patches.rs` injected JS that
   hijacked `/dashboard` and `/wizard` and redirected them to `/admin`, and
   several admin endpoints were unimplemented — so jellyfin-web's requests fell
   through the SPA `fallback_service` and got `index.html` (HTTP 200, `text/html`)
   instead of JSON, crashing those pages on `JSON.parse`.
2. **The custom `/admin` lacked** a logs viewer, an audit/activity log, and a
   devices manager that jellyfin-web's dashboard offers.

Scope was constrained to **real** functionality: parity pages were only added
where they map to genuine Remux config/data. Networking parity was **skipped**
(the `NetworkConfiguration` fields are stored but never honored), and no separate
Transcoding page was added (the existing Settings → Playback page already exposes
every honored `EncodingOptions` field). Quick Connect was already fully
implemented and needed no work.

## Changes

### 1. Un-hijack jellyfin-web admin — `crates/remux-server/src/web_patches.rs`
- Removed the history-interception IIFE that redirected `/dashboard` + `/wizard`
  to `/admin`. Both admin panels are now first-class and reachable.
- Removed the CSS rule hiding jellyfin-web's Plugins sidebar section (it now has
  valid, empty data to show).
- Unrelated behavior patches (maxbitrate shim, async MediaSources loader,
  section-title cleaner, XHR `Fields` shim) are untouched.
- Regression tests assert the hijack/hide stay gone and the other patches remain.

### 2. Missing admin endpoints — new `crates/remux-server/src/api/admin_stubs.rs`
Correctly-shaped **empty** Jellyfin responses so jellyfin-web's admin loads
cleanly (Remux has no plugin system): `GET /plugins`, `/packages`, `/repositories`
(+ no-op `POST /repositories`), `/web/configurationpages`,
`/dashboard/configurationpages`, `/web/configurationpage`, `/notifications/types`,
`/notifications/services`, `/notifications/{user}` (+ `/summary`). Registered in
`api/mod.rs`. Tests assert `401` unauthenticated (not `404`/HTML) + exact shapes.

### 3. Real server file logging + `/System/Logs` — `lib.rs`, `main.rs`, `system.rs`
- New `Config.log_dir` (`Option<String>`, derived `<data_dir>/log` in `resolve()`).
- `setup_logging()` now adds a daily-rolled, ANSI-stripped `remux.log` file layer
  via `tracing-appender` (new dep) and returns a `WorkerGuard` held for the process
  lifetime; the call moved after config load so it can target `log_dir`.
- `GET /system/logs` (list `LogFile[]`) and `GET /system/logs/log?name=` (download
  as `text/plain`, with a path-traversal guard). Tested incl. traversal rejection.

### 4. Activity log (real audit trail)
- New table `activity_log` (migration `202607170002_activity_log.sql`) + DB layer
  `crates/remux-server/src/db/activity_log.rs` (`ActivityLog::record_ignore`/`query`,
  `ActivityKind`/`ActivitySeverity` strum enums).
- `GET /system/activitylog/entries` rewritten from an empty stub to read the table
  with `startIndex`/`limit`/`minDate`/`hasUserId`.
- Recorded at real event sites: login (`users.rs`, incl. Quick Connect), playback
  start/stop (`session.rs`), scheduled-task failure (`tasks/mod.rs`), user
  create/delete (`users.rs`). Recording is fire-and-forget (never fails a request).
- DB-layer + end-to-end tests (login produces an `AuthenticationSucceeded` entry;
  `hasUserId=false` filtering; paging).

### 5. Devices management
- Added a `custom_name` column to `devices` (migration `202607170001…`), threaded
  through `Device` (`db/auth.rs`), `device_info_from` (`models.rs`), and new
  `Device::get_by_id`/`set_custom_name`.
- `GET /devices/info`, `GET/POST /devices/options` (`api/devices.rs`). Round-trip
  test (list → rename → verify → clear → delete).

### 6. Custom `/admin` parity pages — `crates/remux-dashboard`
- New pages: **Logs** (`pages/logs.rs`), **Activity Log** (`pages/activity_log.rs`),
  **Devices** (`pages/devices.rs`), reusing `Card`/`Modal`/`FormGroup`/`states`.
- Renamed the existing "Activity" route (which shows live sessions) to **Sessions**;
  the new **Activity Log** is the audit trail. Added a **System** sidebar group.
- New typed `Endpoint` commands + DTOs in `remux-sdks` (`GetDevices`,
  `SetDeviceOptions`, `DeleteDevice`, `GetLogFiles`, `LogFile`, `DeviceOptions`,
  `GetActivityLog`, `ActivityLogEntry`). Wired in `router.rs` + `layout.rs`.
- `gloo-net` added for the inline log fetch (server accepts the token as `api_key`).
- Pure helpers (`humanize_size`, `log_url`, `severity_color`) are unit-tested.

### 7. Theme system audit — `crates/remux-dashboard/src/theme.rs`
Already complete (8 presets, 18 accents, mode, scale). Added invariant tests
(unique ids, valid hex accents, in-range scales) and a **mechanical registry↔CSS
parity test** (`every_preset_has_a_css_block`) that `include_str!`s `theme.css`.
Documented the pairing rule in `AGENTS.md`.

## Explicitly skipped (not real in Remux)
- **Networking** parity page — all `NetworkConfiguration` fields are inert
  (stored/echoed, never honored; `auth.rs` reads `X-Forwarded-For` unconditionally).
  The GET/POST endpoints stay so jellyfin-web's own page round-trips.
- **Separate Transcoding page** — Settings → Playback already exposes every honored
  encoding option.
- **Plugin/Package/Repository management** — no plugin system; endpoints are
  empty-shape stubs only.

### 8. Admin UI visual refresh — `crates/remux-dashboard`
The `/admin` panel was flat and monotone. Elevated it via a layered section
appended to `assets/theme.css` (derived entirely from existing tokens, so the
theme picker + custom accent still drive it):
- **Server name** now shows in the sidebar brand (with an "Admin" eyebrow),
  breadcrumb root, and dashboard title, instead of a hardcoded "Remux"
  (`layout.rs`, reading `app_state.server.name`).
- **Depth**: cards get real elevation, a glassy top highlight, a hover lift, and
  an accent tick before each title; the background carries a faint accent glow.
- **Dashboard hero**: library counts render as `StatTile`s (big tabular numbers)
  instead of spreadsheet rows (`server_info.rs`; new `fmt_count` thousands
  helper, unit-tested).
- **Active nav** gets an accent spine + wash; primary buttons a subtle gradient.
- Verified in both light and dark themes.

### 9. jellyfin-web admin — deeper fixes found during live verification
- **`SystemInfo.completed_installations`** was `Option<Vec<String>>` serializing as `null`;
  jellyfin-web's dashboard `.map()`s it → crash. Changed to a plain `Vec<String>` so it is
  always `[]` (`remux-sdks`).
- **nginx served `/web/` statically** (`alias /opt/remux/jellyfin-web/`), shadowing jellyfin-web's
  own `/web/ConfigurationPages` API with `index.html` → `r.map is not a function`. Added a
  case-insensitive regex location `~* ^/web/configurationpages?$` that `proxy_pass`es to remux
  (regex wins over the `/web/` prefix) + `Cache-Control: no-store`, and `Cache-Control: no-cache`
  on the static `/web/` block so the SPA shell always revalidates (prevents recurrence; existing
  browsers that cached the broken response need one hard refresh). Backups in
  `/etc/nginx/backups-remux/`.
- Verified live: the jellyfin-web admin dashboard fully renders (server info, devices, storage
  paths, and the Activity panel showing our real activity-log events).

### 10. Sidebar icons + responsive (custom `/admin`)
- **Icons**: an inline-SVG `NavIcon` line-icon set (`components/icons.rs`) wired into every nav
  item/group; brightens on the active item; inherits the accent.
- **Responsive**: drawer breakpoint raised to 900px (iPad portrait gets the roomy drawer),
  `100dvh` sidebar, safe-area insets (notch/home indicator), 16px form fields (no iOS zoom),
  ≥44px touch targets, `overflow-x` scroll for wide row-lists/breadcrumb, 2-col stat grid on
  small phones. Verified on iPhone (390), iPad (1024), and desktop (1440) in light + dark.

### 11. Sidebar hierarchy, naming, and modern form controls (custom `/admin`)
- **Sidebar hierarchy** (`assets/theme.css`): three clear tiers — full-weight top-level items,
  dim letter-spaced section *labels*, and sub-items nested beneath a vertical guide rail whose
  segment turns accent for the active item. Previously headers and sub-items looked identical.
- **Naming** (`layout.rs`): the server name shows once — in the sidebar brand. The top-bar
  dashboard title reads **"Remux"** and the breadcrumb root reads **"Home"** (were duplicating
  the server name).
- **Modern form controls** (`assets/theme.css`): native `<select>` gets a custom chevron
  (`appearance:none`), and checkboxes/radios become accent-driven custom controls with a
  checkmark/dot and focus ring — matching the existing custom toggle switch. Theme-aware; the
  toggle's hidden input is excluded and re-hidden as a safety net.
- Verified live on desktop in light + dark.

## Testing
- `cargo test -p remux-server` — 293 tests (288 pre-existing + new endpoint/db tests), all green.
- `cargo test -p remux-dashboard` — theme + page-helper + `fmt_count` unit tests, all green.
- `dx build --release --package remux-dashboard` — WASM build succeeds.
- **Deployed to production** (`remux.service`, `/usr/local/bin/remux-server` +
  `/opt/remux/dashboard`) with backups of the binary, dashboard, and DB. Verified
  live on remux.obnoxious.lol: `/web/#/dashboard` no longer redirects to `/admin`
  (routes to the jellyfin login preserving `/dashboard`); `/Plugins` → `401
  application/json`, `/web/ConfigurationPages` → `[] 200 application/json`; the
  jellyfin behavior patches remain intact; `/admin` serves the redesigned UI;
  production file logging writes `remux.log`; migrations applied cleanly.
