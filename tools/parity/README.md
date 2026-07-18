# Remux ↔ Jellyfin audio response parity harness

A standalone, re-runnable tool that **proves** Remux's audio JSON responses match a
real Jellyfin's, field by field. It is the regression gate for the audio-parity
work: run it before and after every change to measure gap closure objectively.

## Why a standalone tool (not a `cargo test`)

The repo's in-crate tests (`src/integration_test.rs`, `axum-test`) spin the app up
in-process against fixture data. Parity must instead diff **two live HTTP servers**
— Remux and a real Jellyfin — so it lives here as an operational tool. Colocated
`#[cfg(test)]` unit tests still guard individual serializers (e.g.
`conversions.rs::parity_tests`).

## Reference: throwaway Docker Jellyfin

Because the diff needs Jellyfin's responses for the **same physical files** Remux
serves, spin up a disposable Jellyfin seeded with a copy of a few local albums:

```sh
# 1. copy ~2 albums (rich tags + folder covers) to a seed dir
# 2. run jellyfin, mount the seed read-only
docker run -d --name parity-jf -p 8899:8096 \
  -v "$PWD/jf_config":/config -v "$SEED":/media/music:ro jellyfin/jellyfin:latest
# 3. complete the startup wizard via /Startup/* , create a music library with
#    LibraryOptions.EnableInternetProviders=false (no MusicBrainz IDs to diff),
#    POST /Library/Refresh, poll until Audio count > 0.
# teardown: docker rm -f parity-jf
```

Use `https://demo.jellyfin.org/stable` (user `demo`, empty password) as a secondary
remote-shape reference and for playlists/instant-mix breadth.

## How it works

- **Matching** — items are paired across servers by their **physical file** (Remux
  ids are resolved from `opendal_files` by path), never by Id (which differs).
- **Normalization** — an ignore-list strips server-specific fields (Ids, ServerId,
  ETag, timestamps, image-tag *values*, host/token in URLs, `Path`, `ProviderIds`,
  play counts, Remux-only extras). Everything else is a real parity assertion.
- **Bucketed diff** — `MISSING`, `NULL_VS_VALUE`, `EMPTY_ARRAY_VS_POPULATED`,
  `TYPE_MISMATCH` (incl. key-casing, e.g. `IsAvc`→`IsAVC`), `VALUE_DIFF`, `EXTRA`.
  The first four are hard failures; exit code is non-zero while any remain.

## Usage

```sh
python3 parity.py --profile local    # Remux disk track ↔ seeded Jellyfin (same files)
python3 parity.py --profile remote   # Remux streaming track ↔ demo.jellyfin.org
```

Tokens/ids are read from the scratchpad files written during setup
(`jf_parity_token.txt`, `jf_parity_uid.txt`, `jf_port.txt`, `jfdemo_*`).

## Results (audio track item, 20 identical-file pairs)

| Milestone | Hard gaps |
|---|---|
| Baseline (pre-fix) | **280** (~19 distinct divergences) |
| After Batch A (probe capture + serializer) + Batch B (art/year/disc) | **120** |
| After stream `BitRate`/`Level` fix | **80** |
| After `Genres`/`GenreItems` (entities + batch relation load) | **40** |
| After `HasLyrics` gating + `MediaSource.Name` = file stem | **0 — PASS** |

`python3 parity.py --profile local` → **`RESULT: PASS — 0 real gaps`**, and
`--value-diff` shows no remaining value divergences either.

Closed, each proven by a harness re-run: `Container`, `MediaSources[].{Bitrate,Size}`,
per-stream `BitRate`/`BitDepth`/`Level`/`TimeBase`, `IsAVC` (casing), item-level
`MediaStreams`, `ProductionYear`/`PremiereDate`, `ParentIndexNumber`,
`AlbumPrimaryImageTag` (folder art), `VideoType` (null for audio),
`DisplayTitle`/`IsDefault`, `Genres`/`GenreItems` (MusicGenre entities + a batch
relation loader that fixes empty genres on multi-id browse), `HasLyrics` (gated on
real availability), `MediaSources[0].Name` (file stem, not track title).

### Numeric wire-format note

Jellyfin's `.NET` JSON writer renders a whole-valued `double` without a decimal
(`Level: 0`, not `0.0`). serde_json emits `0.0`, so the SDK's `MediaStream.Level`
uses `serialize_option_whole_f64` (see `remux-sdks/src/lib.rs`) to match byte-for-byte
while keeping the OpenAPI `number` type.
