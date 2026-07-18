#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -eq 0 ]]; then
  versions=(10.11.8 10.11.11)
else
  versions=("$@")
fi
output_dir="${OUTPUT_DIR:-tests/fixtures/jellyfin-music-contract}"
work_dir="$(mktemp -d)"
container="remux-jellyfin-music-contract"
port="${JELLYFIN_CONTRACT_PORT:-18180}"

cleanup() {
  sudo -n docker rm -f "$container" >/dev/null 2>&1 || true
  sudo -n rm -rf "$work_dir"
}
trap cleanup EXIT

mkdir -p "$output_dir" "$work_dir/media/Contract Artist/Contract Album"
ffmpeg -hide_banner -loglevel error -f lavfi -i "sine=frequency=440:duration=2" \
  -c:a flac -metadata title="Complete Track" -metadata artist="Contract Artist" \
  -metadata album_artist="Contract Artist" -metadata album="Contract Album" \
  -metadata genre="Contract Genre" -metadata track="1" -metadata date="2024" \
  "$work_dir/media/Contract Artist/Contract Album/01 Complete Track.flac"
ffmpeg -hide_banner -loglevel error -f lavfi -i "sine=frequency=550:duration=2" \
  -c:a flac -metadata title="Sparse Track" \
  "$work_dir/media/Contract Artist/Contract Album/02 Sparse Track.flac"

auth_header='MediaBrowser Client="ContractAudit", Device="Docker", DeviceId="contract-audit", Version="1.0"'

for version in "${versions[@]}"; do
  instance="$work_dir/$version"
  mkdir -p "$instance/config" "$instance/cache"
  sudo -n docker pull "jellyfin/jellyfin:$version" >/dev/null
  digest="$(sudo -n docker image inspect "jellyfin/jellyfin:$version" --format '{{index .RepoDigests 0}}')"
  sudo -n docker rm -f "$container" >/dev/null 2>&1 || true
  sudo -n docker run -d --name "$container" -p "$port:8096" \
    -v "$instance/config:/config" -v "$instance/cache:/cache" \
    -v "$work_dir/media:/media:ro" "jellyfin/jellyfin:$version" >/dev/null

  base="http://127.0.0.1:$port"
  for _ in {1..90}; do
    curl -fsS "$base/System/Info/Public" >/dev/null 2>&1 && break
    sleep 1
  done
  for _ in {1..30}; do
    curl -fsS "$base/Startup/User" >/dev/null 2>&1 && break
    sleep 1
  done
  curl -fsS -X POST "$base/Startup/User" -H 'Content-Type: application/json' \
    --data '{"Name":"contract","Password":"contract123"}' -o /dev/null
  curl -fsS -X POST "$base/Startup/Complete" -o /dev/null

  auth="$(curl -fsS -X POST "$base/Users/AuthenticateByName" \
    -H 'Content-Type: application/json' -H "Authorization: $auth_header" \
    --data '{"Username":"contract","Pw":"contract123"}')"
  token="$(jq -r .AccessToken <<<"$auth")"
  user_id="$(jq -r .User.Id <<<"$auth")"
  curl -fsS -X POST \
    "$base/Library/VirtualFolders?name=Contract%20Music&collectionType=music&paths=%2Fmedia&refreshLibrary=true" \
    -H "X-Emby-Token: $token" -H 'Content-Type: application/json' --data '{}' -o /dev/null

  for _ in {1..120}; do
    tasks="$(curl -fsS "$base/ScheduledTasks" -H "X-Emby-Token: $token")"
    count="$(curl -fsS "$base/Items?Recursive=true&IncludeItemTypes=Audio&Limit=10" \
      -H "X-Emby-Token: $token" | jq -r .TotalRecordCount)"
    running="$(jq '[.[] | select(.State == "Running")] | length' <<<"$tasks")"
    [[ "$count" -ge 2 && "$running" -eq 0 ]] && break
    sleep 1
  done

  items="$(curl -fsS \
    "$base/Items?Recursive=true&IncludeItemTypes=Audio&Fields=AudioInfo,Genres,MediaSources,People,ProviderIds,Tags&Limit=10" \
    -H "X-Emby-Token: $token")"
  complete_id="$(jq -r '.Items[] | select(.Name == "Complete Track") | .Id' <<<"$items")"
  sparse_id="$(jq -r '.Items[] | select(.Name == "Sparse Track") | .Id' <<<"$items")"
  detail="$(curl -fsS "$base/Items/$complete_id?UserId=$user_id" -H "X-Emby-Token: $token")"
  playback="$(curl -fsS "$base/Items/$complete_id/PlaybackInfo?UserId=$user_id" \
    -H "X-Emby-Token: $token")"
  empty="$(curl -fsS \
    "$base/Items?Recursive=true&IncludeItemTypes=Audio&SearchTerm=contract-no-match&Limit=10" \
    -H "X-Emby-Token: $token")"
  artists="$(curl -fsS "$base/Artists?UserId=$user_id&Limit=1" -H "X-Emby-Token: $token")"
  genres="$(curl -fsS "$base/MusicGenres?UserId=$user_id&Limit=1" -H "X-Emby-Token: $token")"

  lyric_body="$work_dir/lyrics-$version.json"
  lyric_status="$(curl -sS -o "$lyric_body" -w '%{http_code}' \
    "$base/Audio/$sparse_id/Lyrics" -H "X-Emby-Token: $token")"
  stream_headers="$work_dir/stream-$version.headers"
  stream_status="$(curl -sS -D "$stream_headers" -o /dev/null -w '%{http_code}' \
    -H 'Range: bytes=0-0' -H "X-Emby-Token: $token" "$base/Audio/$complete_id/stream")"
  download_headers="$work_dir/download-$version.headers"
  download_status="$(curl -sS -D "$download_headers" -o /dev/null -w '%{http_code}' \
    -H 'Range: bytes=0-0' -H "X-Emby-Token: $token" "$base/Items/$complete_id/Download")"

  jq -n \
    --arg version "$version" --arg digest "$digest" \
    --argjson items "$items" --argjson detail "$detail" --argjson playback "$playback" \
    --argjson empty "$empty" --argjson artists "$artists" --argjson genres "$genres" \
    --argjson lyric_status "$lyric_status" --argjson stream_status "$stream_status" \
    --argjson download_status "$download_status" \
    --arg stream_content_type "$(awk 'BEGIN{IGNORECASE=1} /^content-type:/{gsub("\r",""); sub(/^[^:]+: /,""); print; exit}' "$stream_headers")" \
    --arg stream_accept_ranges "$(awk 'BEGIN{IGNORECASE=1} /^accept-ranges:/{gsub("\r",""); sub(/^[^:]+: /,""); print; exit}' "$stream_headers")" \
    --arg download_disposition "$(awk 'BEGIN{IGNORECASE=1} /^content-disposition:/{gsub("\r",""); sub(/^[^:]+: /,""); print; exit}' "$download_headers")" \
    '
      def present($object; $names): reduce $names[] as $name ({}; .[$name] = ($object | has($name)));
      def music_arrays($item): {
        Artists: $item.Artists, ArtistItems: $item.ArtistItems, AlbumArtists: $item.AlbumArtists,
        Genres: $item.Genres, GenreItems: $item.GenreItems, People: $item.People,
        Tags: $item.Tags, ImageTags: $item.ImageTags, BackdropImageTags: $item.BackdropImageTags,
        ProviderIds: $item.ProviderIds
      };
      ($items.Items[] | select(.Name == "Complete Track")) as $complete |
      ($items.Items[] | select(.Name == "Sparse Track")) as $sparse |
      ($playback.MediaSources[0]) as $source |
      {
        reference: {version: $version, image: $digest},
        query_result_empty: $empty,
        complete_track: music_arrays($complete),
        sparse_track: music_arrays($sparse),
        item_detail_empty_collections: {
          ExternalUrls: $detail.ExternalUrls, Taglines: $detail.Taglines, People: $detail.People,
          Studios: $detail.Studios, RemoteTrailers: $detail.RemoteTrailers, Tags: $detail.Tags,
          LockedFields: $detail.LockedFields, ImageTags: $detail.ImageTags,
          BackdropImageTags: $detail.BackdropImageTags, ProviderIds: $detail.ProviderIds
        },
        browse_shapes: {
          artists: {keys: ($artists | keys | sort), item_type: $artists.Items[0].Type},
          music_genres: {keys: ($genres | keys | sort), item_type: $genres.Items[0].Type}
        },
        playback_info: {
          keys: ($playback | keys | sort),
          source_presence: present($source; ["MediaStreams", "MediaAttachments", "Formats", "RequiredHttpHeaders"]),
          source_collections: {
            MediaStreamsCount: ($source.MediaStreams | length), MediaAttachments: $source.MediaAttachments,
            Formats: $source.Formats, RequiredHttpHeaders: $source.RequiredHttpHeaders
          },
          capabilities: {
            SupportsTranscoding: $source.SupportsTranscoding,
            SupportsDirectStream: $source.SupportsDirectStream,
            SupportsDirectPlay: $source.SupportsDirectPlay,
            SupportsProbing: $source.SupportsProbing
          }
        },
        http: {
          missing_lyrics: {status: $lyric_status},
          audio_stream_range: {status: $stream_status, content_type: $stream_content_type, accept_ranges: $stream_accept_ranges},
          download_range: {status: $download_status, content_disposition: $download_disposition}
        }
      }
    ' >"$output_dir/$version.json"

  sudo -n docker rm -f "$container" >/dev/null
done

printf 'Captured Jellyfin music contracts in %s\n' "$output_dir"
