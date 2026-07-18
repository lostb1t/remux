# Jellyfin music API compatibility audit

Status: implemented and covered by regression tests. Reference captures were generated
from stock Jellyfin 10.11.8 and 10.11.11. Remux currently advertises Jellyfin 10.11.8, so that
version is the compatibility target; 10.11.11 is tracked for drift.

## Reproduction

Run `scripts/capture-jellyfin-music-contract.sh`. The script starts each pinned Jellyfin image,
creates a deterministic two-track music library, waits for the library scan, and writes normalized
contracts under `tests/fixtures/jellyfin-music-contract/`.

Reference images:

| Version | Image digest |
| --- | --- |
| 10.11.8 | `sha256:1694ff069f0c9dafb283c36765175606866769f5d72f2ed56b6a0f1be922fc37` |
| 10.11.11 | `sha256:aefb67e6a7ff1debdd154a78a7bbb780fd0c873d8639210a7f6a2016ad2b35db` |

The normalized contracts for these versions are identical except for the version and image
digest.

## Confirmed contract differences

| Severity | Surface | Stock Jellyfin | Previous Remux behavior | Resolution |
| --- | --- | --- | --- | --- |
| High | Sparse music DTO artist fields | Requested `Artists`, `ArtistItems`, and `AlbumArtists` are present as `[]` | Empty values were changed to `null`/omitted for Finamp | Emit initialized arrays for music items; keep a populated array when an artist exists |
| High | `MediaSourceInfo` collections | `MediaStreams`, `MediaAttachments`, `Formats`, and `RequiredHttpHeaders` are always present | The latter three could be omitted; `MediaAttachments` was not modeled | Model and serialize all four with `[]`/`{}` defaults |
| Medium | Full item detail collections | Empty `ExternalUrls`, `Taglines`, `People`, `Studios`, `RemoteTrailers`, `Tags`, `LockedFields`, `ImageTags`, `BackdropImageTags`, and `ProviderIds` are initialized | Several empty vectors were suppressed by serde | Preserve initialized collection fields in the public DTO |
| Medium | `GET /Artists/{name}` | Returns a `MusicArtist` DTO | Route absent | Add an exact case-insensitive name lookup adapter |
| Medium | `GET /MusicGenres/{name}` | Returns a `MusicGenre` DTO or 404 | Route absent | Add an exact case-insensitive name lookup adapter |

The earlier assumption that stock Jellyfin returns `null` for an empty track artist list was
incorrect. Both audited versions return `[]` when those fields are part of the response. Client
code that crashes on an empty list should be fixed client-side; changing the server to omit the
field breaks clients such as Manet that rely on Jellyfin's initialized shape.

## Music playback contract

### PlaybackInfo JSON

For a successful audio request, stock Jellyfin returns an object with `MediaSources` and
`PlaySessionId`; absent `ErrorCode` is omitted. Each media source initializes:

- `MediaStreams` as an array containing the probed audio stream.
- `MediaAttachments` and `Formats` as empty arrays when unused.
- `RequiredHttpHeaders` as an empty object when unused.
- `SupportsTranscoding`, `SupportsDirectStream`, `SupportsDirectPlay`, and `SupportsProbing` as
  explicit booleans.

Remux now matches those presence and type guarantees. It intentionally does not match the source
values: stock Jellyfin describes a local file, while Remux may describe a dynamically resolved
source and route it through its proxy or HLS pipeline. `Protocol`, `Path`, `TranscodingUrl`,
`HasSegments`, container selection, and stream capabilities must continue to describe the source
Remux can actually serve.

### Stream and download HTTP behavior

The deterministic stock fixture produced these results on both audited versions:

| Request | Status | Relevant headers |
| --- | --- | --- |
| `GET /Audio/{id}/stream` with `Range: bytes=0-0` | 200 | `Content-Type: audio/flac`, `Accept-Ranges: none` |
| `GET /Items/{id}/Download` with `Range: bytes=0-0` | 206 | Attachment disposition with ASCII and RFC 5987 filenames |
| `GET /Audio/{id}/Lyrics` when no lyrics exist | 404 | Problem-details JSON |

Remux's stream endpoint may proxy or transcode and therefore need not copy the first row byte for
byte. Compatibility requirements are the selected route, a playable content type, truthful range
semantics, and stable authentication. Downloads should remain static, support ranges when the
resolved source supports them, and provide an attachment filename.

### Authentication

Music clients commonly authenticate with a `MediaBrowser`/`Emby` authorization header,
`X-Emby-Token`, `X-MediaBrowser-Token`, or the `api_key`/`ApiKey` query parameter. Remux accepts
these forms case-insensitively. A 401 observed on a Manet background `GET /Items?Ids=...` request
cannot be attributed to the `Ids` query contract from server logs alone; capture the request's
authentication headers before changing authorization behavior.

## Route matrix

| Workflow | Remux status |
| --- | --- |
| Browse through `/Items`, `/Users/{userId}/Items`, `/Artists`, `/Artists/AlbumArtists`, and `/MusicGenres` | Implemented |
| Detail through `/Items/{id}` and `/Users/{userId}/Items/{id}` | Implemented |
| Search through `/Search/Hints` | Implemented |
| Playlist create/update/items/reorder/remove | Implemented |
| Playlist share list/add/remove | Partial; only per-user read is implemented |
| Song, album, artist, playlist, item, and named music-genre Instant Mix | Implemented |
| Legacy `/Artists/InstantMix?Id=...` and `/MusicGenres/InstantMix?Id=...` aliases | Implemented as adapters over the existing mix engine |
| Read and remote-search lyrics | Implemented |
| Upload, delete, and download-remote lyrics | Not implemented; these mutate Remux metadata/provider state and need a separate design |
| `PlaybackInfo`, audio stream, universal audio, item file/download | Implemented |

Missing mutating routes are documented rather than emulated with misleading success responses.
They should be added only with persistence and permission semantics that can be defended upstream.

## Regression policy

- Assert field presence separately from value equality. Missing, `null`, `[]`, `{}`, `false`, and
  zero are distinct contracts.
- Do not globally serialize every `Option::None`; stock Jellyfin also suppresses null values.
- Initialize collections in the DTO or endpoint that stock Jellyfin initializes.
- Preserve Remux-specific fields under `Remux` and preserve intentional source-resolution behavior.
- Refresh reference fixtures when the advertised Jellyfin compatibility version changes.
