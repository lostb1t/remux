# Jellyfin API Endpoint Coverage

Legend:
- ✅ real — properly implemented
- 🔧 stub — registered but returns empty / hardcoded data (client won't crash)
- ❌ missing — not registered at all (client gets 404)
- 🚫 n/a — out of scope for this server (music, live TV, plugins, etc.)

Jellyfin web uses **modern** endpoints (no `/Users/{id}/` prefix for playstate/favorites).
Legacy `/Users/{userId}/…` variants are kept for old clients but not the primary target.

---

## ActivityLog

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| 🔧 stub | GET | /System/ActivityLog/Entries | returns empty array |

---

## ApiKey

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Auth/Keys | |
| ✅ real | POST | /Auth/Keys | |
| ✅ real | DELETE | /Auth/Keys/{key} | |
| 🔧 stub | GET | /Auth/Providers | implement: return single default provider |
| 🔧 stub | GET | /Auth/PasswordResetProviders | implement: return single default provider |

---

## Branding

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Branding/Configuration | |
| ✅ real | POST | /Branding/Configuration | via /System/Configuration/Branding |
| ✅ real | GET | /Branding/Css | |
| ✅ real | GET | /Branding/Css.css | |
| ❌ missing | GET | /Branding/Splashscreen | stub: 404 or redirect to placeholder |
| ❌ missing | POST | /Branding/Splashscreen | stub: 204 |
| ❌ missing | DELETE | /Branding/Splashscreen | stub: 204 |

---

## Collection

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ❌ missing | POST | /Collections | stub: create BoxSet — maps to our virtual folder create |
| ❌ missing | POST | /Collections/{collectionId}/Items | stub: 204 (or implement: add item to manual collection) |
| ❌ missing | DELETE | /Collections/{collectionId}/Items | stub: 204 |

---

## Configuration / System

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /System/Configuration | |
| ✅ real | POST | /System/Configuration | |
| ✅ real | POST | /System/Configuration/Branding | |
| ✅ real | GET | /System/Configuration/Network | |
| ✅ real | POST | /System/Configuration/Network | |
| ✅ real | GET | /System/Info | |
| ✅ real | GET | /System/Info/Public | |
| ✅ real | GET | /System/Info/Storage | |
| ✅ real | GET | /System/Endpoint | |
| ✅ real | GET | /System/Ping | |
| ✅ real | POST | /System/Ping | |
| ✅ real | POST | /System/Restart | |
| ✅ real | POST | /System/Shutdown | |
| ❌ missing | GET | /System/Configuration/{key} | stub: 404 |
| ❌ missing | POST | /System/Configuration/{key} | stub: 204 |
| ❌ missing | GET | /System/Configuration/MetadataOptions/Default | stub: return default MetadataOptions |
| ❌ missing | GET | /System/Logs | stub: return empty array |
| ❌ missing | GET | /System/Logs/Log | stub: 404 |
| ❌ missing | GET | /GetUtcTime | implement: return current UTC time |
| ❌ missing | GET | /Tmdb/ClientConfiguration | stub: return empty object |

---

## Devices

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Devices | |
| ❌ missing | DELETE | /Devices | implement: delete device session by id query param |
| ❌ missing | GET | /Devices/Info | stub: return device info by id |
| ❌ missing | GET | /Devices/Options | stub: return empty DeviceOptions |
| ❌ missing | POST | /Devices/Options | stub: 204 |

---

## DisplayPreferences

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /DisplayPreferences/{displayPreferencesId} | |
| ✅ real | POST | /DisplayPreferences/{displayPreferencesId} | |

---

## Environment

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| 🚫 n/a | GET | /Environment/DefaultDirectoryBrowser | not applicable |
| 🚫 n/a | GET | /Environment/DirectoryContents | not applicable |
| 🚫 n/a | GET | /Environment/Drives | not applicable |
| 🚫 n/a | GET | /Environment/NetworkShares | not applicable |
| 🚫 n/a | GET | /Environment/ParentPath | not applicable |
| 🚫 n/a | POST | /Environment/ValidatePath | not applicable |

---

## Filters

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| 🔧 stub | GET | /Items/Filters | returns empty QueryFiltersLegacy |
| ❌ missing | GET | /Items/Filters2 | implement: return available genres/years/ratings from DB |

---

## Genres

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Genres | |
| ❌ missing | GET | /Genres/{genreName} | implement: return single genre item by name |

---

## Images

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Items/{itemId}/Images/{imageType} | redirect to URL |
| ✅ real | GET | /Items/{itemId}/Images/{imageType}/{imageIndex} | redirect to URL |
| ❌ missing | GET | /Items/{itemId}/Images | stub: return empty array |
| ❌ missing | POST | /Items/{itemId}/Images/{imageType} | stub: 204 |
| ❌ missing | POST | /Items/{itemId}/Images/{imageType}/{imageIndex} | stub: 204 |
| ❌ missing | DELETE | /Items/{itemId}/Images/{imageType} | stub: 204 |
| ❌ missing | DELETE | /Items/{itemId}/Images/{imageType}/{imageIndex} | stub: 204 |
| ❌ missing | GET | /UserImage | stub: redirect to placeholder |
| ❌ missing | POST | /UserImage | stub: 204 |
| ❌ missing | DELETE | /UserImage | stub: 204 |
| ❌ missing | HEAD | /Items/{itemId}/Images/{imageType} | implement: same as GET but no body |
| ❌ missing | HEAD | /Items/{itemId}/Images/{imageType}/{imageIndex} | implement: same as GET but no body |
| ❌ missing | GET | /Items/{itemId}/Images/{imageType}/{imageIndex}/{tag}/{format}/{maxWidth}/{maxHeight}/{percentPlayed}/{unplayedCount} | stub: same as simple image redirect |
| ❌ missing | HEAD | /Items/{itemId}/Images/{imageType}/{imageIndex}/{tag}/{format}/{maxWidth}/{maxHeight}/{percentPlayed}/{unplayedCount} | stub: 200 |

---

## Items

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Items | |
| ✅ real | GET | /Items/Latest | |
| ✅ real | GET | /Items/{itemId} | |
| ✅ real | GET | /Items/Counts | |
| ✅ real | POST | /Items/{itemId} | save tags (metadata editor) |
| ✅ real | PATCH | /Items/{itemId} | remux dashboard partial update (non-standard) |
| 🔧 stub | GET | /Items/Suggestions | returns empty |
| 🔧 stub | GET | /Items/{itemId}/Similar | returns empty |
| 🔧 stub | GET | /Items/{itemId}/ThemeMedia | returns stub |
| 🔧 stub | GET | /Items/{itemId}/MetadataEditor | returns empty MetadataEditorInfo |
| ❌ missing | GET | /Items/Root | implement: return root collection folder |
| ❌ missing | DELETE | /Items | stub: 204 (bulk delete — not needed) |
| ❌ missing | DELETE | /Items/{itemId} | implement: delete media item |
| ❌ missing | GET | /Items/{itemId}/Ancestors | implement: return parent chain |
| ❌ missing | GET | /Items/{itemId}/LocalTrailers | stub: return empty array |
| ❌ missing | GET | /Items/{itemId}/SpecialFeatures | stub: return empty array |
| ❌ missing | GET | /Items/{itemId}/ExternalIdInfos | stub: return empty array |
| ❌ missing | GET | /Items/{itemId}/ThemeSongs | stub: return empty QueryResult |
| ❌ missing | GET | /Items/{itemId}/ThemeVideos | stub: return empty QueryResult |
| ❌ missing | GET | /Items/{itemId}/CriticReviews | stub: return empty (legacy, Jellyfin server also stubs this) |
| ❌ missing | GET | /Items/{itemId}/Download | implement: redirect to media URL |
| ❌ missing | GET | /Items/{itemId}/File | stub: same as Download |
| ❌ missing | POST | /Items/{itemId}/Refresh | implement: re-fetch metadata from AIO |
| ❌ missing | POST | /Items/{itemId}/ContentType | stub: 204 |
| ❌ missing | GET | /Items/{itemId}/InstantMix | stub: return empty (music feature) |
| ❌ missing | GET | /Items/{itemId}/Intros | stub: return empty array |
| ❌ missing | GET | /Items/{itemId}/RemoteImages | implement or stub: list fetchable images |
| ❌ missing | GET | /Items/{itemId}/RemoteImages/Providers | stub: return empty array |
| ❌ missing | POST | /Items/{itemId}/RemoteImages/Download | stub: 204 |

---

## Library

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Library/MediaFolders | |
| ✅ real | GET | /Library/VirtualFolders | |
| ✅ real | POST | /Library/VirtualFolders | |
| ✅ real | POST | /Library/VirtualFolders/LibraryOptions | |
| ✅ real | DELETE | /Library/VirtualFolders | |
| ❌ missing | POST | /Library/VirtualFolders/Name | stub: 204 (rename — use PATCH /items/{id} instead) |
| ❌ missing | POST | /Library/VirtualFolders/Paths | stub: 204 |
| ❌ missing | POST | /Library/VirtualFolders/Paths/Update | stub: 204 |
| ❌ missing | DELETE | /Library/VirtualFolders/Paths | stub: 204 |
| ❌ missing | POST | /Library/Refresh | implement: trigger re-import for all catalogs |
| ❌ missing | GET | /Library/PhysicalPaths | stub: return empty array |
| ❌ missing | GET | /Libraries/AvailableOptions | stub: return empty LibraryOptions |
| ❌ missing | POST | /Library/Media/Updated | stub: 204 (webhook) |
| ❌ missing | POST | /Library/Movies/Added | stub: 204 |
| ❌ missing | POST | /Library/Movies/Updated | stub: 204 |
| ❌ missing | POST | /Library/Series/Added | stub: 204 |
| ❌ missing | POST | /Library/Series/Updated | stub: 204 |

---

## LiveTV

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| 🚫 n/a | * | /LiveTv/* | not applicable — return 404 or empty |

---

## Localization

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Localization/Options | |
| ✅ real | GET | /Localization/Countries | |
| ✅ real | GET | /Localization/Cultures | |
| 🔧 stub | GET | /Localization/ParentalRatings | returns empty array |

---

## MediaInfo / LiveStreams

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | POST | /Items/{itemId}/PlaybackInfo | |
| ✅ real | GET | /Items/{itemId}/PlaybackInfo | |
| ❌ missing | POST | /LiveStreams/Open | stub: 404 or minimal response |
| ❌ missing | POST | /LiveStreams/Close | stub: 204 |

---

## Persons

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| 🔧 stub | GET | /Persons | returns empty |
| ❌ missing | GET | /Persons/{name} | implement: return person item by name |

---

## Playback / Playstate

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Playback/BitrateTest | |
| ✅ real | POST | /Sessions/Playing | |
| ✅ real | POST | /Sessions/Playing/Progress | |
| ✅ real | POST | /Sessions/Playing/Stopped | |
| ✅ real | POST | /Sessions/Playing/Ping | |
| ✅ real | POST | /UserPlayedItems/{itemId} | modern endpoint |
| ✅ real | DELETE | /UserPlayedItems/{itemId} | modern endpoint |
| ✅ real | POST | /Users/{userId}/PlayedItems/{itemId} | legacy endpoint |
| ✅ real | DELETE | /Users/{userId}/PlayedItems/{itemId} | legacy endpoint |
| ❌ missing | POST | /UserFavoriteItems/{itemId} | implement: modern favorite (we have legacy only) |
| ❌ missing | DELETE | /UserFavoriteItems/{itemId} | implement: modern unfavorite |
| ❌ missing | POST | /UserItems/{itemId}/Rating | stub: 204 (rating not implemented) |
| ❌ missing | DELETE | /UserItems/{itemId}/Rating | stub: 204 |
| ❌ missing | GET | /UserItems/{itemId}/UserData | implement: return UserItemDataDto |
| ❌ missing | POST | /UserItems/{itemId}/UserData | implement: save user data |
| 🚫 n/a | POST | /PlayingItems/{itemId} | obsolete in Jellyfin source |
| 🚫 n/a | POST | /PlayingItems/{itemId}/Progress | obsolete in Jellyfin source |
| 🚫 n/a | DELETE | /PlayingItems/{itemId} | obsolete in Jellyfin source |

---

## Plugins / Packages / Repositories

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| 🚫 n/a | * | /Plugins/* | not applicable |
| 🚫 n/a | * | /Packages/* | not applicable |
| 🚫 n/a | POST | /Repositories | not applicable |
| 🚫 n/a | GET | /Repositories | stub: return empty array |

---

## QuickConnect

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| 🔧 stub | GET | /QuickConnect/Enabled | returns false |
| ❌ missing | POST | /QuickConnect/Initiate | stub: return error (disabled) |
| ❌ missing | GET | /QuickConnect/Connect | stub: return error (disabled) |
| ❌ missing | POST | /QuickConnect/Authorize | stub: return error (disabled) |
| ❌ missing | POST | /Users/AuthenticateWithQuickConnect | stub: 403 (disabled) |

---

## Remote Images

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ❌ missing | GET | /Items/{itemId}/RemoteImages | implement or stub |
| ❌ missing | GET | /Items/{itemId}/RemoteImages/Providers | stub: return empty array |
| ❌ missing | POST | /Items/{itemId}/RemoteImages/Download | stub: 204 |
| ❌ missing | POST | /Items/RemoteSearch/Apply/{itemId} | stub: 204 |
| ❌ missing | POST | /Items/RemoteSearch/Movie | stub: return empty |
| ❌ missing | POST | /Items/RemoteSearch/Series | stub: return empty |
| ❌ missing | POST | /Items/RemoteSearch/BoxSet | stub: return empty |
| ❌ missing | POST | /Items/RemoteSearch/Person | stub: return empty |
| ❌ missing | GET | /Items/{itemId}/ExternalIdInfos | stub: return empty array |

---

## Scheduled Tasks

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /ScheduledTasks | |
| ✅ real | GET | /ScheduledTasks/{taskId} | |
| ✅ real | POST | /ScheduledTasks/Running/{taskId} | |
| ✅ real | DELETE | /ScheduledTasks/Running/{taskId} | |
| ✅ real | POST | /ScheduledTasks/{taskId}/Triggers | |

---

## Search

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ❌ missing | GET | /Search/Hints | implement: search AIO + DB, return SearchHintResult |

---

## Sessions

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Sessions | |
| 🔧 stub | POST | /Sessions/Capabilities/Full | returns 204 |
| ❌ missing | POST | /Sessions/Capabilities | implement: same as Full but older format |
| ❌ missing | POST | /Sessions/Logout | implement: revoke device token |
| ❌ missing | POST | /Sessions/Viewing | stub: 204 |
| ❌ missing | POST | /Sessions/{sessionId}/Viewing | stub: 204 |
| ❌ missing | POST | /Sessions/{sessionId}/Playing | stub: 204 |
| ❌ missing | POST | /Sessions/{sessionId}/Playing/{command} | stub: 204 |
| ❌ missing | POST | /Sessions/{sessionId}/System/{command} | stub: 204 |
| ❌ missing | POST | /Sessions/{sessionId}/Command | stub: 204 |
| ❌ missing | POST | /Sessions/{sessionId}/Command/{command} | stub: 204 |
| ❌ missing | POST | /Sessions/{sessionId}/Message | stub: 204 |
| ❌ missing | POST | /Sessions/{sessionId}/User/{userId} | stub: 204 |
| ❌ missing | DELETE | /Sessions/{sessionId}/User/{userId} | stub: 204 |

---

## Shows / TV

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Shows/{seriesId}/Seasons | |
| ✅ real | GET | /Shows/{seriesId}/Episodes | |
| 🔧 stub | GET | /Shows/NextUp | returns empty |
| ❌ missing | GET | /Shows/Upcoming | stub: return empty |
| ❌ missing | GET | /Shows/{itemId}/Similar | stub: return empty |

---

## Startup

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Startup/Configuration | |
| ✅ real | POST | /Startup/Configuration | |
| ✅ real | GET | /Startup/User | |
| ✅ real | POST | /Startup/User | |
| ✅ real | POST | /Startup/Complete | |
| 🔧 stub | POST | /Startup/RemoteAccess | returns 204 |
| ❌ missing | GET | /Startup/FirstUser | stub: return same as /Startup/User |

---

## Studios

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ❌ missing | GET | /Studios | implement: return studios from media_relations |
| ❌ missing | GET | /Studios/{name} | implement: return single studio by name |

---

## Subtitles

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Videos/{itemId}/{mediaSourceId}/Subtitles/{index}/subtitles.m3u8 | HLS subtitle playlist |
| ✅ real | GET | /Videos/{routeItemId}/{routeMediaSourceId}/Subtitles/{routeIndex}/Stream.{routeFormat} | subtitle stream |
| ✅ real | GET | /Videos/{routeItemId}/{routeMediaSourceId}/Subtitles/{routeIndex}/{routeStartPositionTicks}/Stream.{routeFormat} | subtitle stream with offset |
| ❌ missing | GET | /Items/{itemId}/RemoteSearch/Subtitles/{language} | stub: return empty |
| ❌ missing | POST | /Items/{itemId}/RemoteSearch/Subtitles/{subtitleId} | stub: 204 |
| ❌ missing | POST | /Videos/{itemId}/Subtitles | stub: 204 |
| ❌ missing | DELETE | /Videos/{itemId}/Subtitles/{index} | stub: 204 |

---

## SyncPlay

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| 🔧 stub | GET | /SyncPlay/List | returns empty |
| 🚫 n/a | GET | /SyncPlay/{id} | not applicable |
| 🚫 n/a | POST | /SyncPlay/* | not applicable |

---

## TimeSync

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ❌ missing | GET | /GetUtcTime | implement: return UtcTimeResponse |

---

## Users

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Users | |
| ✅ real | GET | /Users/Me | |
| ✅ real | GET | /Users/{userId} | |
| ✅ real | GET | /Users/Public | |
| ✅ real | POST | /Users/New | |
| ✅ real | POST | /Users/AuthenticateByName | |
| ✅ real | DELETE | /Users/{userId} | |
| ✅ real | POST | /Users/{userId} | update profile |
| ✅ real | POST | /Users/{userId}/Password | |
| ✅ real | POST | /Users/{userId}/Policy | |
| ✅ real | POST | /Users/{userId}/Configuration | |
| ✅ real | POST | /Users/{userId}/FavoriteItems/{itemId} | legacy |
| ✅ real | DELETE | /Users/{userId}/FavoriteItems/{itemId} | legacy |
| 🔧 stub | GET | /Users/{userId}/GroupingOptions | returns empty |
| 🔧 stub | GET | /Users/{userId}/Items/Resume | returns empty |
| 🔧 stub | GET | /UserItems/Resume | returns empty |
| ❌ missing | POST | /Users | implement: same as /Users/New (newer API uses this path) |
| ❌ missing | POST | /Users/Password | stub: 204 (admin reset password) |
| ❌ missing | POST | /Users/Configuration | stub: 204 (used by newer clients) |
| ❌ missing | POST | /Users/ForgotPassword | stub: return disabled message |
| ❌ missing | POST | /Users/ForgotPassword/Pin | stub: return disabled message |

---

## User Library / Views

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /UserViews | |
| 🔧 stub | GET | /UserViews/GroupingOptions | returns 204 |
| ✅ real | GET | /Users/{userId}/Views | legacy |
| ✅ real | GET | /Users/{userId}/Items | legacy |
| ✅ real | GET | /Users/{userId}/Items/{itemId} | legacy |
| ✅ real | GET | /Users/{userId}/Items/Latest | legacy |
| 🔧 stub | GET | /Users/{userId}/Items/Resume | stub |
| 🔧 stub | GET | /Users/{userId}/Intros | stub |
| 🔧 stub | GET | /Users/{userId}/Items/{itemId}/Intros | stub |

---

## Videos / HLS

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ✅ real | GET | /Videos/{itemId}/stream | |
| ✅ real | GET | /Videos/{itemId}/stream.{container} | |
| ✅ real | GET | /Videos/{itemId}/master.m3u8 | |
| ✅ real | GET | /Videos/{itemId}/main.m3u8 | |
| ✅ real | GET | /Videos/{itemId}/main/stream.m3u8 | |
| ✅ real | GET | /Videos/{itemId}/main/{segmentFile} | |
| ✅ real | GET | /Videos/{itemId}/hls1/{playlistId}/{segmentFile} | |
| ✅ real | DELETE | /Videos/ActiveEncodings | |
| 🔧 stub | GET | /Videos/{itemId}/AdditionalParts | returns empty |
| ❌ missing | HEAD | /Videos/{itemId}/stream | implement: 200 with headers only |
| ❌ missing | HEAD | /Videos/{itemId}/stream.{container} | implement: 200 with headers only |
| ❌ missing | HEAD | /Videos/{itemId}/master.m3u8 | implement: 200 with headers only |
| ❌ missing | GET | /Videos/{itemId}/live.m3u8 | stub: 404 (live TV only) |
| ❌ missing | GET | /Videos/{itemId}/hls/{playlistId}/stream.m3u8 | stub: map to main/stream.m3u8 |
| ❌ missing | GET | /Videos/{itemId}/hls/{playlistId}/{segmentId}.{segmentContainer} | stub: map to hls1 handler |
| ❌ missing | GET | /Videos/MergeVersions | stub: 204 |
| ❌ missing | DELETE | /Videos/{itemId}/AlternateSources | stub: 204 |
| ❌ missing | GET | /Videos/{videoId}/{mediaSourceId}/Attachments/{index} | stub: 404 |

---

## Years / Studios / Persons (metadata grouping)

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| ❌ missing | GET | /Years | implement: distinct years from media DB |
| ❌ missing | GET | /Years/{year} | implement: return year as BaseItemDto |
| ❌ missing | GET | /Studios | implement: from media_relations |
| ❌ missing | GET | /Studios/{name} | implement: from media_relations |
| ❌ missing | GET | /Persons/{name} | implement: person by name |

---

## Music (all out of scope)

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| 🚫 n/a | * | /Audio/* | music streaming — not applicable |
| 🚫 n/a | * | /Artists/* | not applicable |
| 🚫 n/a | * | /MusicGenres/* | not applicable |
| 🚫 n/a | GET | /Songs/{itemId}/InstantMix | not applicable |
| 🚫 n/a | * | /Playlists/* | not applicable (for now) |
| 🚫 n/a | * | /InstantMix routes | not applicable |
| 🚫 n/a | * | /FallbackFont/* | not applicable |
| 🚫 n/a | * | /Trickplay/* | not applicable (for now) |
| 🚫 n/a | * | /MediaSegments/* | not applicable |

---

## Misc (n/a or low value)

| Status | Method | Path | Notes |
|--------|--------|------|-------|
| 🚫 n/a | * | /Backup/* | not applicable |
| 🚫 n/a | * | /Plugins/* | not applicable |
| 🚫 n/a | * | /Packages/* | not applicable |
| 🚫 n/a | GET | /Repositories | stub: return empty array |
| 🚫 n/a | * | /LiveTv/* | not applicable |
| 🚫 n/a | * | /Channels/* | stub: return empty |
| 🚫 n/a | * | /web/ConfigurationPage* | dashboard served separately |
| 🚫 n/a | POST | /ClientLog/Document | stub: 204 |

---

## Priority queue — what to implement next

### 🔴 High (breaks Jellyfin web UX if missing)

1. `POST /Sessions/Logout` — sign-out revokes device token
2. `POST /UserFavoriteItems/{itemId}` + `DELETE` — modern favorites (web client uses these, not the legacy `/Users/{id}/FavoriteItems`)
3. `GET /Items/Root` — root folder (used during library browse init)
4. `GET /Items/{itemId}/Ancestors` — breadcrumb trail in item detail view
5. `GET /Search/Hints` — search bar in Jellyfin web
6. `GET /Items/Filters2` — filter panel (genres, years, ratings from actual DB)
7. `GET /GetUtcTime` — client clock sync (some clients call this on startup)
8. `POST /Sessions/Capabilities` — plain capabilities (not just Full variant)
9. /System/ActivityLog/Entries 

### 🟡 Medium (degrades experience)

9. `DELETE /Items/{itemId}` — delete a media item from admin
10. `POST /Items/{itemId}/Refresh` — re-fetch metadata from AIO
14. `POST /Library/Refresh` — trigger re-import of all enabled catalogs
15. `GET /Studios` + `GET /Studios/{name}` — studio browsing
16. `GET /Years` + `GET /Years/{year}` — year-based browsing
17. `GET /Persons/{name}` — person detail page
18. `GET /Genres/{genreName}` — genre detail page
19. `DELETE /Devices` — remove session/device

### 🟢 Low (stubs are fine)

21. All `/Collections/*` — box set management (stub 204)
22. All `/Library/VirtualFolders/Paths` variants — stub 204
23. `GET /Shows/Upcoming` — stub empty
24. `POST /Sessions/{sessionId}/*` — remote control commands (stub 204)
25. `POST /Sessions/Viewing` — stub 204
26. `GET /System/Configuration/{key}` — named config (stub 404)
27. `POST /Users/ForgotPassword` — stub "disabled"
28. `POST /Users/AuthenticateWithQuickConnect` — stub 403
29. `GET /Branding/Splashscreen` — stub 404
30. All image HEAD variants
31. `GET /Libraries/AvailableOptions` — stub empty
32. `GET /Library/PhysicalPaths` — stub empty
33. All `/Library/Movies/Updated` webhook stubs — stub 204
20. `GET /Items/{itemId}/RemoteImages` — image picker in metadata editor
11. `GET /Items/{itemId}/LocalTrailers` — stub returning empty []
12. `GET /Items/{itemId}/SpecialFeatures` — stub returning empty []
13. `GET /Items/{itemId}/ExternalIdInfos` — stub returning empty []