# Remux playback verification harness

A standalone, re-runnable tool that **proves** the server actually serves a
*decodable audio stream* for a track — not merely a `200` — and that albums and
playlists resolve every child to a playable source. It is the regression gate for
music playback.

## What it checks (per track)

Walking the real client flow:

1. `POST /Items/{id}/PlaybackInfo` → pick a MediaSource + PlaySessionId, and
   sanity-check its shape (`Protocol`, `IsRemote`, `Container`, an `Audio`
   MediaStream, direct-play/transcode support).
2. Fetch the stream three ways with a `Range` header, asserting `200/206` + an
   `audio/*` Content-Type + non-empty body:
   - `/Audio/{id}/universal` — **and asserts it does not redirect into the video
     HLS pipeline** (`/videos/{id}/master.m3u8`). That regression made every audio
     track hand a music client a *video* manifest; this check locks the fix in.
   - `/Audio/{id}/stream?static=true`
   - `/Items/{id}/File`
3. `--deep` (implicit for `--item`): download the whole stream and decode it with
   `ffmpeg -f null -` — proving the bytes are a valid, fully decodable audio file,
   and reporting codec / sample-rate / channels / duration.

Exit code is non-zero on any hard failure, so it works as a CI/regression gate.

## Usage

```sh
# one track, deep decode test
python3 verify.py --item <track-id>

# every track on an album / every member of a playlist
python3 verify.py --album <album-id>
python3 verify.py --playlist <playlist-id> [--limit N]

# a random library sample, or an explicit id list ("<id>|<name>" per line)
python3 verify.py --sample 40
python3 verify.py --idfile local_ids.txt
```

Config via env (`REMUX_BASE`, `REMUX_TOKEN`, `REMUX_UID`, `REMUX_DB`); defaults
target the local server.

## Findings it has proven (2026-07-16)

- **`/Audio/{id}/universal` redirected audio into the video pipeline**
  (`→ /videos/{id}/master.m3u8?VideoCodec=copy&AudioCodec=aac`), so a music client
  received a 192-byte video HLS manifest for an audio-only track. Fixed to redirect
  to the range-capable direct audio stream. *(playback.rs::audio_universal)*
- **Single-item `/Items/{id}` mislabeled every local track** as `Protocol:Http` /
  `IsRemote:true` with a video transcoding URL (the unconditional Track wrap in
  `item()`); local files now serve a direct `File` source, matching the batch path
  and PlaybackInfo. *(items.rs)*
- After both fixes: the Hayley Williams album is **19/19**, a 40-track random local
  sample is **40/40**, all decode-verified.

## Streaming→local healing (implemented)

The `car time` playlist is **entirely streaming-backed** (Deezer rows); at test time
~27% failed because the signed upstream URL was dead (`410`) or the track was gone
upstream (empty `PlaybackInfo`). **111 of its 133 members have an identical local
file.** The opendal-local addon now matches a streaming track to a local copy strictly
on **title + album + artist** (against the reliable `media` table, single unambiguous
candidate only) and serves the local file — it is a primary provider, so the reliable
local copy wins. Result: `car time` went **97/133 → 125/133**, all healed tracks
decode-verified, zero regressions. The 8 residual failures have no local copy (gone
upstream) or point at a since-deleted file (stale index). See
`MUSIC-CLIENT-COMPAT-AUDIT.md` and `crates/remux-server/src/addons/opendal.rs`.
