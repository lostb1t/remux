# PINNED PR DRAFT — Remux playlist album/artist metadata

Status: **ready, NOT pushed.** Handle later. Do not push to `main` — PR branch only.

## Git state
- Branch: `fix/playlist-items-artist-album` (commit `418d282`, rebased onto current `origin/main`)
- Author: `Bobbls <25752872+Bobbls@users.noreply.github.com>` (set repo-locally in the worktree)
- Worktree: `/tmp/remux-pr` (ephemeral; the branch ref + objects persist in `/opt/remux-server/.git`)
- Two files: `crates/remux-server/src/api/playlists.rs`, `crates/remux-server/src/api/items.rs`
- Verified: `cargo check` clean on this base; fix is live-deployed + verified on the running Remux.

## Push plan (when ready)
Push branch to `origin` (lostb1t/remux-server — Bobbls has push access) using the PAT in
`/opt/jellyflix/.git-remux-creds`, then open the PR as Bobbls (gh/API). One squashed commit.

## PR title
fix: populate album/artist on playlist items

## PR body
playlist tracks come back with empty Album / AlbumArtist / Artists / ArtistItems; only
AlbumId is populated. The same track fetched on its own, or listed under its album, has
all of it.

Cause: db_media_to_item derives a track's album from media.parent.title, its album id from
media.parent_id, and its artist fields from media.grandparent. Both playlist listing paths
(get_playlist_items, and the playlist branch of get_items) build their items with
db::Media::get_by_id, which loads only the row and its images, not the parent/grandparent
relations. So everything db_media_to_item derives from those relations comes out empty;
only AlbumId survives, since it reads the parent_id column rather than the relation. Album
and hierarchy listings avoid this because get_by_jellyfin_filter already calls
preload_parents.

Fix: do the same in both playlist paths. Collect the page's tracks, run preload_parents over
the batch to hydrate parent/grandparent, then build the DTOs (pairing each track with its
playlist relation id to keep order).
