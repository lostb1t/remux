Status: local notes only. Do not upstream blindly.

## Where Things Live

- Main local repo:
  `/opt/remux-server`
- PR-ready worktrees:
  - `/tmp/remux-server-pr-clean`
  - `/tmp/remux-server-subtitle-proof`
  - `/tmp/remux-server-sibling-proof`
- Current local-only maintenance commit:
  - `9e9410d fix(test): order media_relations index after schema creation`

These notes are meant to answer two questions later:

- which changes are strong upstream PR candidates
- which changes were only local maintenance or operational mitigation

## Ready

### `pr/resumed-ts-hls-clean`

- Worktree: `/tmp/remux-server-pr-clean`
- Commit: `699f9b2`
- Scope: resumed TS-HLS playlist serving
- Why it exists:
  resumed TS-HLS sessions could start ffmpeg at the correct non-zero position
  while still serving a synthetic zero-based variant playlist, which caused the
  client to request `segment_00000.ts` and effectively restart from zero.
- Why it belongs in Remux:
  the mismatch was between Remux's transcode session state and the playlist it
  served back to the client.
- Proof:
  targeted unit coverage exists on the branch, and the fix was validated against
  local Jellyflix playback/audit runs where resumed startup stopped regressing.

### `pr/subtitle-alias-proof`

- Worktree: `/tmp/remux-server-subtitle-proof`
- Commit: `555878b`
- Scope: subtitle codec alias matching in both `device_profile.rs` and the
  `playbackinfo` image-subtitle support check
- Why it exists:
  Remux device-profile matching treated values like `pgs` and
  `hdmv_pgs_subtitle` as different codecs even though they are effectively the
  same subtitle format family in client capability matching. A second strict
  equality check in `api/playback.rs` could still inject
  `SubtitleCodecNotSupported` for the selected subtitle stream even after the
  profile layer accepted the alias.
- Why it belongs in Remux:
  this is Remux's own direct-play, subtitle-delivery, and playback-decision
  logic.
- Proof:
  upstream-clean base failed a direct unit repro:
  `subtitle_delivery_method_accepts_pgs_aliases`
  Focused playback repro also failed before the playback-side fix:
  `test_playbackinfo_accepts_pgs_aliases_for_selected_subtitle`
  with `SubtitleCodecNotSupported`.
  After the full patch set:
  `device_profile::tests::` passed, and the focused playback repro passed on a
  proof base that included the local migration-ordering fix.

### `pr/sibling-probe-proof`

- Worktree: `/tmp/remux-server-sibling-proof`
- Commit: `2b4bb8c`
- Scope: `/sessions` should not borrow subtitle/audio `MediaStreams` from a
  sibling source when a concrete selected source still exists but lacks probe
  data.
- Why it exists:
  the old fallback logic could make `NowPlayingItem.MediaStreams` describe a
  different source than the one actually selected for playback.
- Why it belongs in Remux:
  the incorrect data is generated inside Remux session hydration; Jellyflix only
  sees the already-wrong payload.
- Proof:
  focused integration repro added on the branch:
  `test_get_sessions_does_not_borrow_sibling_streams_for_selected_unprobed_source`
  The test was run both ways:
  old unconditional sibling borrowing fails by exposing the sibling subtitle
  track, and the guarded behavior passes.

## Local Maintenance Only

### Migration ordering fix

- Local commit: `9e9410d`
- Scope: move `idx_media_relations_right_left` creation so it happens after
  `media_relations` exists.
- Why it exists:
  authenticated Rust tests were failing before reaching playback logic because
  migration `202606090001_audio_features.sql` tried to create an index on
  `media_relations` before `202606140005_squash.sql` created that table.
- Why this is not a PR candidate right now:
  user explicitly considers this a local maintenance issue rather than an
  upstream-worthy change.
- Validation:
  these previously broken tests now pass locally:
  `test_get_sessions_with_active_session`
  `test_subtitle_search_item_not_found`

### Local Rust build workaround for optional `jemalloc`

- Scope: local validation only
- Repo fact:
  `crates/remux-server/Cargo.toml` has `default = ["jemalloc"]`
- Repeated local issue:
  `cargo build` / `cargo test` can fail inside `tikv-jemalloc-sys` before
  reaching the Remux logic under test
- Practical workaround used during this audit:
  - `cargo test -p remux-server --no-default-features <test-name> -- --nocapture`
  - `cargo build -p remux-server --no-default-features`
- Why this note exists:
  this failure mode keeps reappearing and should not be confused with the
  playback/API bug currently being investigated
- Why this is not a PR by itself:
  this is documented as local operator/developer guidance, not an upstream
  product change

### Live operational mitigation

- Scope: `/sessions` timeout under load
- Observed shape on June 21-22, 2026:
  live `/sessions` timed out while Remux logs showed repeated
  `pool timed out while waiting for an open connection`
- Cause found locally:
  SQLite pool max is `5`, while live `MetaConcurrency` had been set to `25`
- Local mitigation applied:
  lowered live `MetaConcurrency` from `25` to `4`
- Why this is not a PR candidate right now:
  this is currently best explained as a local configuration mismatch causing
  pool starvation, not a proven Remux code defect
- Validation:
  `/sessions?device_id=...` immediately recovered after the config change

## Unproven / Do Not Upstream Yet

### `settings.rs` in-process config cache

- Current location:
  uncommitted change in `/opt/remux-server/crates/remux-server/src/db/settings.rs`
- Intended benefit:
  reduce repeated `settings` table reads for hot paths like playback/session
  requests
- Why it is not PR-ready:
  there is no proof yet that this is the right fix layer for the live timeout
  issue, and it introduces process-local staleness semantics that need real
  review
- Current judgment:
  treat as an unproven local experiment until it has a focused repro, benchmark,
  and clear invalidation story

### Unrelated local draft

- `PR-DRAFT-playlist-metadata.md` is an older separate Remux fix note
- It is not part of the current remux-compatibility / playback audit
