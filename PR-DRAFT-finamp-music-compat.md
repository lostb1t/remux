# PR DRAFT — Finamp music compatibility: browse crash, local-library grouping, playback resilience

Status: **implemented + built.** Server-side fixes for three Finamp-beta breakages plus a
diagnosed client-side issue. Verified against the live Remux instance (see each section's
verification). Do not push to `main` — PR branch only.

## Background

Finamp Beta (the `redesign` branch of `finamp-app/finamp`) surfaced several breakages against
Remux's Jellyfin-compatible API. Each is a distinct root cause; they are documented and fixed
independently below. Issues A, B, C are server-side and fixed here. Issue D is a Finamp client
bug; the server change that reduces its triggers overlaps with B/C.

---

## A — `Unsupported operation: Wrong BaseItemDto type: MusicArtist`

**Symptom.** Browsing the music library in Finamp throws and blanks the view.

**Root cause.** Finamp force-wraps the children of a library browse into concrete typed
classes (`Album.fromItem` / `Track.fromItem` in `lib/models/music_models.dart`), which throw
on any item whose type isn't the wrapper's. Remux returned `MusicArtist` index nodes mixed
into an unfiltered music-library browse: for a `CollectionMediaKind::Music` smart collection,
`api/items.rs` defaulted `IncludeItemTypes` to `[Track, Album, Artist]` when the client sent
none, so `GET /Items?ParentId={musicLib}` returned artists alongside albums and songs.
Canonical Jellyfin never surfaces bare `MusicArtist` nodes in a library's child listing —
artists are a virtual index reached only via `/Artists`.

**Fix.** `crates/remux-server/src/api/items.rs` — in the "no `IncludeItemTypes` supplied"
branch, filter `MusicArtist` out of the default type set. The explicit-filter path is
untouched, so `/Artists`, `/Artists/AlbumArtists`, and `IncludeItemTypes=MusicArtist` still
return artists. (Removing `Artist` from the base `collection_types` instead would have broken
`/Artists` when it carries a `ParentId`, because that vector is reused as the intersection
constraint for explicit requests.)

**Verification.** Unfiltered `GET /Items?ParentId={musicLib}` now returns
`TotalRecordCount == Audio + MusicAlbum` exactly (artists excluded), a random 3 000-item
sample contains zero `MusicArtist`, while `/Artists?ParentId=…` and
`IncludeItemTypes=MusicArtist` still return the full artist count.

---

## C — Local music (self-released / underground) never appears (e.g. "9TAILS ARCHIVE")

**Symptom.** Whole folders of on-disk music never show up in Finamp; ~18.8k local tracks were
invisible; the library had only ~460 artists for 100k+ tracks.

**Root cause.** `opendal-local` scanning creates bare `Track` rows (no `parent_id` album, no
`grandparent_id` artist, no probe). The only thing that assigns a track's album/artist is the
**Deezer** enrichment (`addons/deezer.rs`), and the enrichment loop never sees tracks: both
feeders — `RefreshLibrary` via `Media::get_refreshable` (`kind IN (Movie, Series)`) and
`RefreshAllMeta` (`Movie/Series/Artist`) — exclude `Track`. Deezer-native catalog tracks
arrive pre-grouped at import, but local files, and anything Deezer can't match (underground /
self-released artists like *9TAILS*, *sadeyes*), stay orphaned forever.

**Fix.** A new library task, `GroupLocalMusic`
(`crates/remux-server/src/tasks/group_local_music.rs`), groups orphaned local tracks using the
files' own embedded tags:

- Selects `opendal-local` tracks whose media row has no `parent_id`, joined to `opendal_files`
  for the on-disk path.
- Reads `album_artist` / `artist` / `album` / `title` / `track` tags via `ffprobe`
  (`FFPROBE_PATH`, already a permitted bootstrap env var), bounded to 8 concurrent probes.
  Falls back to the folder structure only when a tag is missing. Library roots for the path
  fallback are read from the addon config at runtime — never hardcoded to a server's layout.
- Synthesizes `Artist` and `Album` rows with deterministic (stable-UUID) ids and **merges into
  an existing artist/album of the same normalized name**, so local tracks attach to an existing
  Deezer artist where one exists (and a later Deezer pass can enrich the synthesized ones with
  canonical artwork). Relinks each track's `parent_id`/`grandparent_id`/`parent_idx`.

Supporting change: `crates/remux-server/src/db/media.rs` — `Media::validate()` previously
required `deezer_artist` (Artist) / `deezer_album` (Album), which made a local-only artist/album
unrepresentable. Both now also accept a `custom_stremio_id`, mirroring how `Track` already
accepts one for local content. Operator control: `Config::group_local_music_limit` caps a run
for staged validation.

**Verification.** Unit tests (`cargo test … group_local_music`, 6 passing) cover normalization,
year/tag stripping, track-number parsing, path fallback, and tag-precedence. Post-run: orphan
track count drops toward zero, artist count rises, and a "9TAILS ARCHIVE" artist/album becomes
browsable via `/Artists?SearchTerm=…`.

---

## B — `(-1008) resource unavailable` on playback under load

**Symptom.** Some tracks fail to play, intermittently, especially when opening a full album or
queue.

**Root cause.** ~99.8% of tracks store no source and are resolved live per play through the
"Monochrome" stream addon → an external worker → a signed URL. The worker rate-limits (HTTP
429) under concurrency; `addons/eclipse.rs` turned a 429 straight into an error via
`error_for_status()?`, which the caller
(`services/stream_service.rs::load` → `refresh_streams`) swallowed, leaving the track with no
source → HTTP 500 → Finamp `-1008`. There was no concurrency cap, no retry/backoff, and no
request timeout, so a queue open self-inflicted a burst of 429s.

**Fix.** `crates/remux-server/src/addons/eclipse.rs`:

- A `WORKER_CONCURRENCY` semaphore (mirrors the db layer's `DB_WRITE_SEMAPHORE`) caps in-flight
  worker requests so an album/queue open can't fan out into a 429 storm.
- Worker GETs go through `worker_get_json`, which retries transient failures (429s, network
  blips) with exponential backoff + jitter via the project's `retry!` macro instead of failing
  permanently on the first 429.

`crates/remux-server/src/addons/mod.rs` — the shared addon HTTP client now sets request and
connect timeouts, so a stalled upstream can't pin a resolution indefinitely; the timeouts are
operator-tunable via `Config::addon_http_timeout_secs` /
`Config::addon_http_connect_timeout_secs` (threaded through the addon `from_cfg` constructors).

**Known follow-up (not in this change).** A subset of Deezer-imported albums have zero child
tracks (e.g. the *sadeyes* "8pm" album), so they are unplayable and also feed issue D. That is
a separate import-path data defect; fixing it safely needs dedicated work and testing against
the live catalog, so it is documented here rather than patched blind.

**Verification.** Concurrent `PlaybackInfo` for N tracks of one artist should no longer return
500s under load; a single track still resolves end-to-end (PlaybackInfo → `/Audio/{id}/universal`
→ master playlist → segment 200).

---

## D — Finamp download shows a white screen (diagnosed; client-side)

**Root cause (client).** Not a server bug. Finamp's downloads screen reads a **synchronous**
provider (`userDownloadedItemsProvider`) with no error boundary and force-unwraps
`stub.baseItem!` in `lib/components/DownloadsScreen/downloaded_items_list.dart`. When a
downloaded/enumerated item has a null `baseItem` — which Remux produces via 0-track albums and
tracks that fail to resolve (500 / `-1008`) — the null-check throws during `build()` and Flutter
renders a blank (white) screen. This is the same defect class as Finamp issue
[#748](https://github.com/finamp-app/finamp/issues/748) (partially fixed by PR #749, which
stopped the type enum from throwing but left the render-layer `!` unwraps). The download path
does **not** use `/scheduledtasks` (its repeated 401 is unrelated and admin-gated by design).

**Server mitigation.** The complete fix is upstream in Finamp (null-safety + filtering). Server
side, issue B's resolver resilience reduces the no-source tracks that produce null items, and
resolving the 0-track-album follow-up above would remove the other trigger.

---

## Files changed (server)

| File | Issue | Change |
|---|---|---|
| `crates/remux-server/src/api/items.rs` | A | Exclude `MusicArtist` from the no-filter music browse |
| `crates/remux-server/src/tasks/group_local_music.rs` | C | New `GroupLocalMusic` task (with unit tests) |
| `crates/remux-server/src/tasks/mod.rs` | C | Register the task |
| `crates/remux-server/src/db/media.rs` | C | Allow `custom_stremio_id` for Artist/Album in `validate()` |
| `crates/remux-server/src/addons/eclipse.rs` | B | Worker concurrency cap + retry/backoff |
| `crates/remux-server/src/addons/mod.rs` | B | HTTP client timeouts (config-driven) |
| `crates/remux-server/src/addons/{stremio,lrclib}.rs` | B | Thread `Config` into `make_http_client` |
| `crates/remux-server/src/lib.rs` | B, C | `Config` fields: `group_local_music_limit`, `addon_http_timeout_secs`, `addon_http_connect_timeout_secs` |
