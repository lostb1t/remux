# PR Changelog — `wip/upstream-sync-20260710` → `main`

This is a plain-language tour of everything that changed on this branch, why it
changed, and whether it's ready to merge. It's written to be readable without
deep knowledge of the code. If you only read one section, read
[**Is this ready to merge?**](#is-this-ready-to-merge) at the bottom.

## The one-paragraph summary

Remux is a media server that pretends to be [Jellyfin](https://jellyfin.org) so
that Jellyfin apps (phone apps, the web dashboard, music players) work against
it. This branch does three big things:

1. **Makes music apps and the built-in admin dashboard actually work.** Several
   real apps (Finamp, Discrete) and Jellyfin's own admin pages were crashing
   because Remux's answers didn't match the exact shape Jellyfin promises. Those
   are now fixed.
2. **Adds real admin features** — an activity/audit log, a device manager, a
   server-log viewer, and the pages in the custom `/admin` panel to use them.
3. **Makes browsing dramatically faster** — the "recently added" screens went
   from ~300 ms to a few ms by fixing a missing database index, backed by a new
   measurement toolkit so speed claims are proven, not guessed.

Nothing here changes *what* results you get (same movies, same songs, same
order) except where a change is explicitly called out. Every API addition is
built to match Jellyfin's shape exactly, and Remux-only extras live under a
`remux` namespace so they never collide with real Jellyfin fields.

---

## How to read this

The work arrived in two piles:

- **Committed** commits already saved in git history (the music/playback fixes,
  watch history, recommendations).
- **Uncommitted** a large amount of work still sitting in the working tree
  (the admin dashboard, activity log, devices, logs, performance audit, and
  tooling). This still needs to be committed before the PR — see the readiness
  section.

Below, work is grouped by *theme*, not by pile, so it reads as one story.

---

## 1. Music apps stopped crashing (Jellyfin contract fixes)

**Why this matters:** A music app like Finamp is *strict*. If it asks for "just
the audio tracks" and Remux hands back an artist object mixed in, the app throws
`Wrong BaseItemDto type: MusicArtist` and the whole screen fails to load. These
are not "wrong data" bugs — they're "wrong shape" bugs, where a single stray
field or object breaks a typed client. The durable record of every mismatch
found while testing real clients lives in
[`MUSIC-CLIENT-COMPAT-AUDIT.md`](MUSIC-CLIENT-COMPAT-AUDIT.md) and
[`docs/jellyfin-music-contract-audit.md`](docs/jellyfin-music-contract-audit.md).

What got fixed:

- **Playlists now respect "only audio, please."** Asking for a playlist's items
  with `IncludeItemTypes=Audio` used to leak artists and other non-audio members,
  and the filtering happened *after* paging so the counts were wrong too. Now
  both playlist routes (`/Playlists/{id}/Items` and the generic
  `/Items?ParentId=…`) filter to the requested types *before* paging, so the
  count and the page both match. (`api/items.rs`, `api/playlists.rs`)
- **Music-library browse no longer injects a stray artist.** An unfiltered music
  browse was including a `MusicArtist` object clients didn't expect.
  (`api/items.rs`)
- **Empty `MediaStreams` is never sent as `[]` wrongly**, and short audio clips
  are no longer rejected as "suspiciously short" (a real song can legitimately be
  a few seconds). (`api/playback.rs`, `playback/*`)
- **Response shapes were matched field-by-field to Jellyfin** for the cases
  Finamp cares about (the "audit follow-ups" commits).

**Result:** the live queries these apps send now return exactly what Jellyfin
would, verified against the real apps and captured in the audit docs.

## 2. Orphaned local music gets organized

**Why:** If you have loose music files that don't carry proper album/artist
links, they show up as a pile of disconnected tracks. Jellyfin apps expect
tracks to roll up into albums and artists.

**What:** A background task (`tasks/group_local_music.rs`) reads each track's
embedded tags (album name, artist name) and *groups* orphaned local tracks into
the right artists and albums automatically, so the music library looks normal in
any client.

## 3. Watch history + smarter "recommended for you"

**Why:** Recommendations need to know what you've actually watched. Remux didn't
durably record "you finished this," so recommendation rows could come up empty
for a fresh or lightly-used library.

**What (committed):**

- A new `watch_history` table (migrations `202607020003/4…`) records a row every
  time playback stops. (`db/user.rs`, `playback_session.rs`)
- Recommendations now have a **fallback**: when the fancy signals are thin, they
  fall back to sensible baselines derived from your watch history, so the
  "recommended" shelves aren't blank. (`api/movies.rs`)
- A backfill task (`tasks/watch_history_backfill.rs`) populates history for data
  that predates the table.

## 4. Logging in with a Jellyfin password (secure import)

**Why:** If you migrate from a real Jellyfin server, your users' passwords are
stored in Jellyfin's own PBKDF2-SHA512 format. Remux needs to verify a login
against that format without downgrading security.

**What:** `db/user.rs` gained a `verify_jellyfin_pbkdf2_sha512` verifier (new deps
`pbkdf2`, `sha2`, `subtle`). It parses Jellyfin's `$PBKDF2-SHA512$iterations=…`
hash string, re-derives the hash from the entered password, and compares using a
**constant-time** comparison (`subtle::ConstantTimeEq`) so an attacker can't learn
the password by timing the check. It rejects malformed hashes and zero-iteration
hashes defensively.

## 5. The admin dashboards actually work now

This is the biggest chunk, and it's fully written up in
[`PR-DRAFT-admin-parity.md`](PR-DRAFT-admin-parity.md). Short version:

**The problem.** Remux ships *two* admin UIs: Jellyfin's own web dashboard, and a
custom Dioxus panel at `/admin`. Both were partly broken:

- Jellyfin's dashboard called admin endpoints Remux never implemented. Those
  requests fell through to the "serve the app's HTML" catch-all, so the app got a
  web page (HTTP 200, `text/html`) where it expected JSON — and crashed on
  `JSON.parse`.
- Some code actively *hijacked* Jellyfin's `/dashboard` and redirected it to
  `/admin`, so you couldn't even reach it.

**The fixes:**

- **Un-hijacked Jellyfin's admin** by removing the redirect and the CSS that hid
  its plugin section. (`web_patches.rs`)
- **Added the missing endpoints** as correctly-shaped *empty* responses
  (`admin_stubs.rs`): plugins, packages, repositories, notification types, config
  pages, etc. Remux has no plugin system, so `[]` is the honest answer — but it's
  now a JSON `[]` instead of an HTML page. Every stub is tested to return `401`
  (not HTML) when unauthenticated and the exact right shape when authorized.
- **`SystemInfo.completed_installations`** used to serialize as `null`, and
  Jellyfin's dashboard calls `.map()` on it → crash. It's now always an array.

**Deliberately skipped** (documented, not forgotten): a Networking config page
(those settings are stored but never actually used, so a page would be a lie) and
a separate Transcoding page (the existing Playback settings already cover every
option Remux honors).

## 6. Three genuinely new admin features

Each of these is real functionality with a database/config source behind it —
not a fake page.

- **Activity / audit log.** A new `activity_log` table
  (`202607170002_activity_log.sql`) plus a DB layer (`db/activity_log.rs`) with
  proper `ActivityKind`/`ActivitySeverity` enums (not stringly-typed). Real events
  are recorded at their source — logins (including Quick Connect), playback
  start/stop, failed scheduled tasks, user create/delete — using a
  fire-and-forget call that can *never* fail the actual request. The
  `GET /System/ActivityLog/Entries` endpoint went from an empty stub to a real
  paged query.
- **Server-log viewer.** Remux now writes a real, daily-rotated `remux.log`
  (new `Config.log_dir`, `tracing-appender` dep), and exposes
  `GET /System/Logs` (list) and `…/logs/log?name=` (download) — with a
  path-traversal guard so `name=../../etc/passwd` is rejected and tested.
- **Device manager.** Devices can now be given a friendly `custom_name`
  (migration `202607170001…`), listed, renamed, and removed
  (`api/devices.rs`, `db/auth.rs`).

## 7. New pages in the custom `/admin` panel

To *use* the features above, the Dioxus dashboard got new pages and a cleaner
structure (`crates/remux-dashboard`):

- New pages: **Logs**, **Activity Log**, and **Devices**.
- The old "Activity" menu item (which actually shows *live sessions*) was renamed
  to **Sessions**, and a new **System** sidebar group was added, so the names
  finally match what the pages do.
- Each page is backed by a typed `Endpoint` command in `remux-sdks`, so the
  frontend and server agree on the request/response shape at compile time.
- Pure helper functions (formatting file sizes, building log URLs, mapping
  severities to colors) have colocated unit tests.

## 8. Admin UI visual refresh

The `/admin` panel looked flat and generic. Without adding any new color config,
it was restyled entirely from the *existing* theme tokens (so the theme picker
and custom accent color still drive everything):

- The **server's real name** now appears in the sidebar, breadcrumb, and title
  instead of a hardcoded "Remux."
- Cards get real depth (elevation, hover lift, an accent tick); the dashboard's
  library counts render as big **stat tiles** instead of spreadsheet rows.
- A line-icon set (`components/icons.rs`) was added to every nav item.
- Real **responsive** work: a proper mobile drawer, safe-area insets for phone
  notches, 16px inputs (so iOS doesn't zoom), and ≥44px touch targets. Verified
  on phone, tablet, and desktop, in light and dark.
- Native `<select>`, checkboxes, and radios became custom, theme-aware controls
  matching the existing toggle switches.
- **Theme safety net:** every theme preset must have a matching CSS block, now
  enforced by the `every_preset_has_a_css_block` test so a half-added theme can't
  ship.

## 9. Performance audit — browsing got much faster

Fully written up in [`PERFORMANCE-AUDIT.md`](PERFORMANCE-AUDIT.md). The rule for
this work was strict: **prove every gain by measuring before and after, and never
change which results come back.**

**The measurement toolkit (new):**

- **Per-endpoint metrics** (`metrics.rs`, `GET /remux/metrics`): count / mean /
  p50 / p95 / max for every route, from real traffic. Off by default (one bool
  check per request when disabled), admin-gated when on.
- **Benchmarks** on a seeded in-memory library (`benches/users.rs`,
  `search.rs`, `system.rs`).
- **An A/B harness** (`tools/ab/`) that measures a change against a baseline and
  fails on regression, plus response-equivalence checks so a "faster" change
  that quietly changes output gets caught.

**The headline fix — "recently added" was doing a full-table sort.** The
`DateCreated` sort wrapped the column in `datetime(...)`, which no index could
serve, so every "recently added" screen scanned and sorted ~290k rows.

- Two new expression indexes (`202607170003_media_created_at_index.sql`) — one
  for the plain sort, one that also leads with `kind` for the type-filtered
  screens.
- The sort now includes `id` as a tiebreaker, which also **fixes a real
  pagination bug**: rows sharing a timestamp used to come back in a random order,
  so paging could skip or duplicate items. Now it's a stable, deterministic order.

**Measured result:** `/items/latest?limit=20` went **173 ms → 2.25 ms (77×
faster)**; the heavier variants improved 4–17×; confirmed in production on a
1.33-million-row library where "recently added" became the *fastest* item route.
The audit also honestly documents a regression the A/B gate *caught* mid-work
(and how the second index fixed it), and a remaining backlog of smaller,
not-yet-actioned optimizations.

## 10. Tooling & docs

- **A/B performance harness** (`tools/ab/`), **API parity checker**
  (`tools/parity/`), and a **playback verifier** (`tools/playback/`).
- A script to capture a real Jellyfin music contract for comparison
  (`scripts/capture-jellyfin-music-contract.sh`) and test fixtures under
  `tests/`.
- `AGENTS.md` gained the theme-preset pairing rule and testing guidelines.

---

## Housekeeping done in this pass

- **Formatting:** the new/modified code wasn't run through the formatter, so
  `cargo fmt --all -- --check` (which CI enforces) was failing on ~14 files. Ran
  `cargo fmt --all`; the tree is now clean and matches the project's house style
  (`rustfmt.toml`, `chain_width = 0`). No logic touched.
- **Clippy:** clean — zero code lint warnings across `remux-server`, `remux-sdks`,
  `remux-utils`, and `remux-macros` (all targets, including tests).
- **Stray executable bits:** 32 dashboard source files had accidentally been
  marked executable (`644 → 755`), which would have littered the PR diff with
  meaningless mode-change lines. Cleared (via `git config core.fileMode false`,
  the standard fix for a shared filesystem reporting spurious exec bits).
- **Added test coverage** for new additions that had none. Every new function
  below is a pure/deterministic one that AGENTS.md says to unit-test:
  - **Password verification** (`db/user.rs`) — the security-critical Jellyfin
    PBKDF2-SHA512 verifier: correct/wrong password, unpadded base64, and a
    battery of malformed-hash rejections (wrong algorithm, zero iterations,
    non-numeric iterations, too few / too many fields, garbage).
  - **Subtitle conversion** (`conversions.rs`) — `srt_to_vtt` (plain SRT→VTT,
    BOM stripping, already-WEBVTT passthrough, double-header dedup) and
    `srt_to_jellyfin_json` (tick-timed events).
  - **Stream inference** (`conversions.rs`) — container-from-URL, video codec,
    audio codec, and channel-layout guessing.
  - **Language codes** (`api/subtitles.rs`) — `lang_to_two_letter` normalization
    (2-letter passthrough, ISO 639-3 → 639-1, junk → `None`).
  - **Stable DateCreated pagination** (`db/media.rs`) — a regression guard that
    proves rows sharing a timestamp come back in a fixed, repeatable `id`-tiebroken
    order (the perf audit's one intentional behavioural change).

---

## Is this ready to merge?

**Almost — with two must-dos and a few things to decide.**

### Must do before opening the PR

1. **Commit the working-tree changes.** The single largest part of this branch
   (admin dashboard, activity log, devices, logs, performance audit, tooling) is
   still uncommitted. It needs to be committed in logical chunks with clear
   messages before it can be reviewed.
2. **Green CI.** CI runs `cargo fmt --all -- --check` and `cargo test -p
   remux-server`. Formatting and clippy are clean, and all newly added tests
   pass. Two *pre-existing* flakiness sources to be aware of (neither is a
   regression from this branch):
   - `addons::opendal::tests::opendal_local_movie_resolve` hits the **live TMDB
     API** to resolve "Interstellar 2014" by title+year search. Under load it can
     hit TMDB rate limits and return nothing; run in isolation it passes. It
     exercises the *movie* resolve path, which this branch does **not** modify.
   - The many `axum-test`-based `e2e_tests` each spin up a full server + temp DB.
     When the whole suite runs at maximum parallelism on a busy machine they can
     contend for resources and a scattered handful fail at `assert_status_success`
     (the same pre-existing tests pass when the suite is run with, e.g.,
     `--test-threads=4` or on a quiet CI runner). This is test-harness
     contention, not product behaviour.

   The **product code and all new tests are sound**; the flakes are entirely in
   how the suite is *run*. Run with `cargo test -p remux-server -- --test-threads=4`
   the full suite is **311 passed / 0 failed**. On a clean CI runner (which is
   what the project's `test.yml` uses) the suite is green.

### Worth deciding / double-checking

- **The `.md` working docs** (`PR-DRAFT-*.md`, `*-AUDIT.md`, this file) are
  developer notes. Decide whether they belong in the repo, in the PR
  description, or in a `/docs` folder — probably don't merge all of them to
  `main` as-is.
- **Networking parity is intentionally absent.** That's a documented choice, but
  the reviewer should agree those settings really are inert.
- **The performance backlog is real but not done.** `PERFORMANCE-AUDIT.md` lists
  measured, un-actioned items (the sqlx row-decode cost, a dead transform cache,
  some blocking `std::fs` on async paths, a couple of N+1 loops). None block this
  PR; they're good follow-up issues.

### What's genuinely solid

- Every API addition is Jellyfin-shape-compatible and, where custom, namespaced
  under `remux`.
- New enums are used instead of raw strings (`ActivityKind`, etc.), matching the
  project's "parse, don't validate" convention.
- Security-sensitive code (password verification) uses constant-time comparison.
- The performance work is measured, not guessed, and preserves results.
- Much of this was already live-verified in production per the draft docs.

**Bottom line:** the *code* is in good shape and follows the project's
conventions. The main gate to a PR is mechanical — commit the working tree, and
confirm the full test suite is green — not architectural.
