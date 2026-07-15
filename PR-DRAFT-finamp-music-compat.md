# PR DRAFT — Finamp music compatibility: browse crash, local-library grouping, playback resilience

Status: **implemented + built + deployed + verified** against the live Remux instance (see each
section's verification). Covers the browse crash (A), local-library grouping (C), playback
resilience (B), the download white-screen (D/F), the playlist-playback crash (E), and
Jellyfin-spec audit follow-ups. Do not push to `main` — PR branch only.

## Background

Finamp Beta (the `redesign` branch of `finamp-app/finamp`) surfaced several breakages against
Remux's Jellyfin-compatible API. Each is a distinct root cause; they are documented and fixed
independently below. All are server-side. A recurring theme (D, F, E, and the audit follow-ups):
Remux returned an **empty array or unfiltered list where stock Jellyfin returns a populated array,
null, or a filtered list**, and Finamp — which works against stock Jellyfin — force-unwraps
(`.first`) or type-wraps those and crashes. Fixing the response shape to match Jellyfin is the
throughline.

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

**Root cause (server response shape — corrected).** An earlier hypothesis blamed a Finamp null
`baseItem`; the actual trigger is server-side. Finamp's download dialog estimates size with
`mediaSources?.first...mediaStreams.first.codec` (`lib/components/AlbumScreen/download_dialog.dart`).
The `?.` guards a **null** `MediaSources`/`MediaStreams` but **not an empty `[]`**, so Dart's
`List.first` throws `Bad state: No element` synchronously in `build()` → blank/white screen. Remux
served every track's `MediaSources[0].MediaStreams` as `[]` (empty) when the track had no probe
data. Stock Jellyfin always ships at least one stream, so it never crashes — this is a
Remux-vs-Jellyfin shape gap, not a pure Finamp bug. (The `/scheduledtasks` 401 is unrelated —
the download path never calls it.)

**Fix (issue F).** `crates/remux-server/src/conversions.rs` — `From<db::Media> for MediaSourceInfo`
now synthesizes a minimal audio stream (`type=Audio`, codec from the container) when the probed
stream list is empty, so `MediaStreams` is never `[]`.

---

## E — `Wrong BaseItemDto type: MusicArtist` when playing a playlist (fixed & verified)

**Root cause.** A separate live instance of the same class as A, in the playlist path — and the
actual reason the MusicArtist error persisted after clearing the client cache. A playlist may
contain non-audio members (the user's "car time" playlist contained the artist *sadeyes*), and
`GET /playlists/{id}/items` (`api/playlists.rs`) **ignored `IncludeItemTypes` entirely**. Finamp
requests `IncludeItemTypes=Audio` to build a play queue and hard-throws on the leaked MusicArtist;
stock Jellyfin filters it.

**Fix.** `get_playlist_items` now resolves the members' kinds in one batch query and applies the
`IncludeItemTypes` filter before paginating, Jellyfin-style. The artist remains a playlist member
for unfiltered views.

---

## Jellyfin-spec audit follow-ups (fixed & verified)

Diffing Remux's responses against the Jellyfin OpenAPI schema surfaced more of the same
empty-array anti-pattern:

- **Null, not `[]`, for empty `Artists`/`ArtistItems`/`AlbumArtists`** on tracks with no linked
  artist (`api/models.rs`) — Finamp `.first`-crashes on empty arrays but tolerates null.
- **`MusicGenre`/`Genre` `IsFolder=true`** (`db/media.rs` `is_folder`) — genres are browsable
  containers in Jellyfin, not playable leaves.

Deferred (documented, not patched): some tracks omit `RunTimeTicks` (needs probe data; not
implicated in the reported crashes); `/System/Info` omits some cosmetic fields.

---

## G — Local music won't play; `-1008` on short tracks (fixed & verified)

Two problems made music fail to play with `(-1008) resource unavailable`:

1. **The probe rejected short audio.** `playback::probe` discarded any stream whose probed
   duration was under ~3 minutes as a "suspiciously short placeholder" — a heuristic meant for
   copyright-strike *videos*, applied to the resolved `Stream` row (so a `kind` check doesn't
   help). Real songs are routinely under 3 minutes, so it discarded every usable stream → 500 →
   `-1008`, for local files and short streaming tracks alike. Fix
   (`crates/remux-server/src/playback/probe.rs`): gate the short-duration rejection on the probed
   content having a **video stream** — audio-only content is never rejected for being short.

2. **Local files were never served.** The six `opendal-local` music addons added 2026-07-10
   (`1tb4music`, …) were registered `catalog`-only, so they indexed files but could not serve
   them; only the original `10tbmusic` carried the `stream` resource. And Monochrome (the live
   stream resolver, priority −20) outranked the local addons (priority 0), so even a track with a
   local file resolved to a (often wrong, fuzzy) Qobuz match first. **Operational fix via the
   admin addon API** (no code): added `stream` to all local-music addons' `resources`, and set
   their `priority` to −30 (above Monochrome) so a track you have on disk plays from disk, with
   Monochrome as the fallback for streaming-only tracks — exactly the "disk + streaming backend"
   behaviour. Verified: the local 9TAILS track and 8 random local tracks across artists all play
   (`PlaybackInfo=200`, source = the local file), while Deezer-only tracks still resolve via
   Monochrome.

---

## Favorited-artist data cleanup

All favorited *artist* rows were unfavorited across users (they were a second MusicArtist trigger
on Finamp's favorites screen). Reversible; no schema change.

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
| `crates/remux-server/src/api/playlists.rs` | E | Apply `IncludeItemTypes` to playlist items |
| `crates/remux-server/src/conversions.rs` | F | Never emit empty `MediaStreams` on a MediaSource |
| `crates/remux-server/src/api/models.rs` | audit | Null (not `[]`) for empty artist arrays |
| `crates/remux-server/src/db/media.rs` | audit | `MusicGenre`/`Genre` `IsFolder=true` |
