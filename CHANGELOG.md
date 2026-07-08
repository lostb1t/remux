# [0.13.0](https://github.com/lostb1t/remux/compare/v0.12.1...v0.13.0) (2026-07-08)


### Bug Fixes

* **addons:** accumulate meta patches into clean object so highest-priority addon wins ([#87](https://github.com/lostb1t/remux/issues/87)) ([f5ecc5d](https://github.com/lostb1t/remux/commit/f5ecc5d7a30c01e46f5e84fac29d52d86a7b5c6f))
* admit kitsu-only anime and ensure stable UUID for kitsu items with no IMDB ID ([48a3019](https://github.com/lostb1t/remux/commit/48a30196320b08966489e5a21bfc3ab7b1e514df))
* skip malformed TMDB guest star entries missing id/name ([8f09e89](https://github.com/lostb1t/remux/commit/8f09e89919008b400754e2af47c6cd12094d11ba))
* userviews now reflects user-defined collection order ([478fc48](https://github.com/lostb1t/remux/commit/478fc48fe3a5c6e977bcf9a95ab7c5df9f521bc2))


### Features

* add Kitsu ID resolution and guard against error meta leaking into DB ([772e488](https://github.com/lostb1t/remux/commit/772e488828b5f92eacc5ecefdbf371c328166fc9))


### Performance Improvements

* optimize meta refresh ([#89](https://github.com/lostb1t/remux/issues/89)) ([ec4ffc8](https://github.com/lostb1t/remux/commit/ec4ffc83d588913f11a3e34561b0d1e0660b04f4))

## [0.12.1](https://github.com/lostb1t/remux/compare/v0.12.0...v0.12.1) (2026-07-05)


### Bug Fixes

* **opendal:** normalize dotted/bracketed dir names before SKIP_DIRS check, fixes [#83](https://github.com/lostb1t/remux/issues/83) ([2fb114d](https://github.com/lostb1t/remux/commit/2fb114d6c63300cc8488d71af275da6a4ebcc909))
* **streams:** resolve encode resolution instead of source-disc label for UHD BluRay filenames ([24d43b6](https://github.com/lostb1t/remux/commit/24d43b687346263eecfab16e6cdd894c55ecd6a8))
* **transcode:** normalize AudioStreamIndex < 0 to None at transcode boundary ([fcf623a](https://github.com/lostb1t/remux/commit/fcf623aa3eb6a3cfce17fda69f73d89b7715571f))
* **web_patches:** guard against non-string itemId in patched getItem ([ffd36c5](https://github.com/lostb1t/remux/commit/ffd36c58a3fcc92e08bc1e89225eeeeeaee45851))

# [0.12.0](https://github.com/lostb1t/remux/compare/v0.11.0...v0.12.0) (2026-07-03)


### Bug Fixes

* **auth:** improve legacy Jellyfin header fallback handling ([#77](https://github.com/lostb1t/remux/issues/77)) ([ec8226e](https://github.com/lostb1t/remux/commit/ec8226e07c5f83fa969c24bd2bc8a344ae64f5b1))
* **items:** implement metadata editor, item update, and content type endpoints ([284a0bf](https://github.com/lostb1t/remux/commit/284a0bf1f63631880c2839b38685085f3ee74288))
* **people:** skip episode credits.cast, add GuestStar API type, use TMDB order for cast weight ([94feffc](https://github.com/lostb1t/remux/commit/94feffc078ade0edd3885d3c81e918d2d67c9071))
* **popularity:** improve trending algorithm with weighted split-window scoring ([7ca73eb](https://github.com/lostb1t/remux/commit/7ca73ebd2ebe7abb247491f3c4f5922202b862cf))
* set streams_refreshed_at early in refresh to prevent date shifting when inserts are slow ([b7006f2](https://github.com/lostb1t/remux/commit/b7006f2038f50e19bd12164673d1828697211d0e))


### Features

* **migrations:** remove duplicate episode cast already present on series ([e0b21b1](https://github.com/lostb1t/remux/commit/e0b21b11bcbbb17e8c3eaf0d58ded412b44feadf))
* **tasks:** enum-driven task categories with consistent sort order ([f0a7e97](https://github.com/lostb1t/remux/commit/f0a7e978bd13e093c60656b19fc9b54b7104f62e))

# [0.11.0](https://github.com/lostb1t/remux/compare/v0.10.2...v0.11.0) (2026-07-01)


### Bug Fixes

* force meta refresh on catalog import so configured meta providers are always applied ([deb167e](https://github.com/lostb1t/remux/commit/deb167e24bf55a7af2df00b078394da98270062b))
* import local TV when source advertises an Episode catalog ([#64](https://github.com/lostb1t/remux/issues/64)) ([bce45dd](https://github.com/lostb1t/remux/commit/bce45ddcd481d55ee6f20c217a47049e1324264e))
* **opendal:** derive episode series title from directory when filename starts with episode code ([5af1f62](https://github.com/lostb1t/remux/commit/5af1f6206cf652735a162c06d42a1a2050420d24))
* refactor playback  ([#69](https://github.com/lostb1t/remux/issues/69)) ([5d96e80](https://github.com/lostb1t/remux/commit/5d96e8016f0b920e2aa82837225809e81285cdb9))
* **sessions:** populate SeriesName and SeasonName in NowPlayingItem ([#62](https://github.com/lostb1t/remux/issues/62)) ([d526ebd](https://github.com/lostb1t/remux/commit/d526ebd0c31fcc02df07a54dc8fdaab3870e7738))
* skip unreleased episodes when marking season/series played with release filter enabled ([#41](https://github.com/lostb1t/remux/issues/41)) ([#42](https://github.com/lostb1t/remux/issues/42)) ([e0ecd2d](https://github.com/lostb1t/remux/commit/e0ecd2d1363b6d8589a585d412137f010b0e49d2))
* **subtitles:** honor codec aliases in playback decisions ([#44](https://github.com/lostb1t/remux/issues/44)) ([5c72159](https://github.com/lostb1t/remux/commit/5c721595991341196da7443a310753ed523de7f1))
* **transcode:** only apply dovi_rpu bsf for confirmed Dolby Vision streams ([#50](https://github.com/lostb1t/remux/issues/50)) ([20a3071](https://github.com/lostb1t/remux/commit/20a30712500efc196e6d36a8896ecd1fdcef0542))
* **web:** strip Recently Added inu prefix from homescreen row titles ([4413464](https://github.com/lostb1t/remux/commit/4413464a9cb5239ac19c299ff3db3652ded21537))


### Features

* add destructive flag to Task trait with confirmation modal for purge tasks ([3541250](https://github.com/lostb1t/remux/commit/3541250efd451f6cef8633b2bd90a6d3e1d4fb59))
* add filter rule groups with AND/OR nesting ([#55](https://github.com/lostb1t/remux/issues/55)) ([3bbadb9](https://github.com/lostb1t/remux/commit/3bbadb9ec15086dfef3ce80b8949337d61da711f))
* add support for mixed (movie and shows) collections ([#54](https://github.com/lostb1t/remux/issues/54)) ([620c6c3](https://github.com/lostb1t/remux/commit/620c6c32c2750c28ccb1cbdcd7750b5106689ee6))
* add TMDB popular, top rated and trending catalogs ([f5c5820](https://github.com/lostb1t/remux/commit/f5c5820e05c1eb7bb7a8f5b6a1df8b96d85f909b))
* add TMDB watch provider tags to movie and series metadata ([737fb03](https://github.com/lostb1t/remux/commit/737fb0329b175b0b61c6d142f8847d86458908a9))
* **deezer:** surface playlists as real Jellyfin playlists ([#60](https://github.com/lostb1t/remux/issues/60)) ([906cc36](https://github.com/lostb1t/remux/commit/906cc367712ccd76e814a0763a207bf1e84825de))
* extend TMDB search addon to support Movie and Series kinds ([fc6e62e](https://github.com/lostb1t/remux/commit/fc6e62ef20ad2410dde6b70798acfeddd7fe0215))
* make tmdb addon an system addon ([13e4b24](https://github.com/lostb1t/remux/commit/13e4b24710d2fe07c450f627a4cd97a9c18a1613))
* **meta:** Added language, studios and production locations including filters ([#52](https://github.com/lostb1t/remux/issues/52)) ([9b779d4](https://github.com/lostb1t/remux/commit/9b779d4f7b08762052a53ce40e8aae8aeecb0e10))
* popularity metrics ([#57](https://github.com/lostb1t/remux/issues/57)) ([2b8df7b](https://github.com/lostb1t/remux/commit/2b8df7b1b9633b07ce8da472aacb09aeaa709c9b))
* trakt addon with metrics ([#65](https://github.com/lostb1t/remux/issues/65)) ([ed6ee58](https://github.com/lostb1t/remux/commit/ed6ee5834e0e347d6f046c99ed14c87bb5e49e85))


### Performance Improvements

* skip relations batch-load when Fields doesn't include People/Genres/Studios ([b5d6775](https://github.com/lostb1t/remux/commit/b5d6775ca46b804abd3f69baf0a981a03c74add6))

## [0.10.2](https://github.com/lostb1t/remux/compare/v0.10.1...v0.10.2) (2026-06-24)


### Bug Fixes

* **web:** fix stream loading flicker on playback ([e0e4cc0](https://github.com/lostb1t/remux/commit/e0e4cc06b55ab658562cab67b5fe8ed822701e22))
* **web:** race conditions in async stream handling ([5fd3f2f](https://github.com/lostb1t/remux/commit/5fd3f2ff6f1d79f1032b6671031a4ddf0497a1bc))

## [0.10.1](https://github.com/lostb1t/remux/compare/v0.10.0...v0.10.1) (2026-06-24)


### Bug Fixes

* **hls:** use EXT-X-START for resumed TS-HLS instead of ffmpeg playlist ([#51](https://github.com/lostb1t/remux/issues/51)) ([8ca8fd3](https://github.com/lostb1t/remux/commit/8ca8fd33150d82711b0e7a723117902ad8633165))


### Performance Improvements

* load streams async on item details page for web ([#47](https://github.com/lostb1t/remux/issues/47)) ([9fcbd8d](https://github.com/lostb1t/remux/commit/9fcbd8d69fb45d727f368c7493fe46e0d9374acc))

# [0.10.0](https://github.com/lostb1t/remux/compare/v0.9.0...v0.10.0) (2026-06-23)


### Bug Fixes

* apply release filter to nextup ([#35](https://github.com/lostb1t/remux/issues/35)) ([#38](https://github.com/lostb1t/remux/issues/38)) ([78472b3](https://github.com/lostb1t/remux/commit/78472b33ab47374035940ef08cd89d42638ec586))
* hide recent theatrical-only movies until digital release confirmed ([e4f7eb1](https://github.com/lostb1t/remux/commit/e4f7eb1bbf414fe112af21dae0285c453ce8d698))
* **hls:** serve ffmpeg playlist for resumed ts-hls ([#43](https://github.com/lostb1t/remux/issues/43)) ([66c1ed1](https://github.com/lostb1t/remux/commit/66c1ed14e41b5b2e52fb99fb35ca1e64245f010b))
* **images:** proxy external image URLs instead of redirecting ([6076c56](https://github.com/lostb1t/remux/commit/6076c56e64e9527c818ec65309037722465f3504))
* override collection sort when SortName appears anywhere in sort list ([c1231d4](https://github.com/lostb1t/remux/commit/c1231d42790d345d141bc498d37e82af6e4d0788))
* prevent squash migration from re-running on every restart ([#36](https://github.com/lostb1t/remux/issues/36)) ([7c80a12](https://github.com/lostb1t/remux/commit/7c80a124c592bd5c1dc7de5585beeb8d464f06b2))
* return empty when includeItemTypes doesn't match collection content type ([a7f0d5b](https://github.com/lostb1t/remux/commit/a7f0d5b9e550c646b5ac069758f3d463a5fa6830))


### Features

* intro support ([#32](https://github.com/lostb1t/remux/issues/32)) ([#39](https://github.com/lostb1t/remux/issues/39)) ([ba23b31](https://github.com/lostb1t/remux/commit/ba23b310be9719d3f9c941e81f3e25137ad9ac28))


### Performance Improvements

* **images:** use sized TMDB image variants and populate ImageTags.Thumb ([9f6c4a5](https://github.com/lostb1t/remux/commit/9f6c4a540c9e23be011125291a6edb7fd33e0c11))

# [0.9.0](https://github.com/lostb1t/remux/compare/v0.8.0...v0.9.0) (2026-06-21)


### Bug Fixes

* external subtitles for web ([aa335f1](https://github.com/lostb1t/remux/commit/aa335f10fa4817e827620fb847d76f2f18e0b904))
* force nextup active-series join order ([#26](https://github.com/lostb1t/remux/issues/26)) ([fefccdf](https://github.com/lostb1t/remux/commit/fefccdf7f6608513a47f44032eb42322a8e00c9c))
* handle progress reports without play session id ([#29](https://github.com/lostb1t/remux/issues/29)) ([5833a3f](https://github.com/lostb1t/remux/commit/5833a3f6679b7d0863cae8f57beea819fcb14539))
* inherit runtime from ([a7a53cb](https://github.com/lostb1t/remux/commit/a7a53cbf6b1ba401bf24b1f5b32d754f4eb3fa07))
* nextup was missing imported nedia [#14](https://github.com/lostb1t/remux/issues/14) ([5ec7471](https://github.com/lostb1t/remux/commit/5ec7471655db82523d1c37e1203423a6f23cd971))
* optimize iptv purge ([4c64a89](https://github.com/lostb1t/remux/commit/4c64a89d4ed50020252855585d79d2ab3999057e))
* order continue watching by play date ([#19](https://github.com/lostb1t/remux/issues/19)) ([17ac545](https://github.com/lostb1t/remux/commit/17ac5454fcb714df12a61b81819a8aef9e5d61a9))
* pass --repo to gh release create to avoid missing git context ([99ad721](https://github.com/lostb1t/remux/commit/99ad721477228dbec84a45c8f75c83f9109632e5))
* persist probe data between stream refresh ([f4212dd](https://github.com/lostb1t/remux/commit/f4212dd5b18696fcd72ac271566fd252642356ac))
* query paramaters wrongly encoded resulting in wrong tmdb calls ([ef0ef77](https://github.com/lostb1t/remux/commit/ef0ef77a2b731cc2b07724875c59870295821cef))
* respect enable_user_data and normalize NextUp cutoff handling ([#21](https://github.com/lostb1t/remux/issues/21)) ([9270125](https://github.com/lostb1t/remux/commit/9270125e869a68b0c6f3f53de6a133a9e1b8350b))
* set DeliveryUrl on subtitle streams, respect device profile ([ac6c83b](https://github.com/lostb1t/remux/commit/ac6c83bb45698f63f65648b1adfce8c79c232ae4))
* use source bitrate as encoding target, cap at max_streaming_bitrate ([d17203e](https://github.com/lostb1t/remux/commit/d17203e5617f9381386f3045a2b098ee9e541f51))
* wrongly returning zero on items list with results ([d1125f1](https://github.com/lostb1t/remux/commit/d1125f16de8d32d8042ff9e8ddb707d6b96e0385))


### Features

* force plezy to reload versions ([4855d56](https://github.com/lostb1t/remux/commit/4855d560d0aed6ce9e9d2b734e15e3d7c9d1b2ab))
* Implement AudioLanguagePreference and RememberAudioSelections user settings ([46d1284](https://github.com/lostb1t/remux/commit/46d1284f6def125332707c58bd9bd035cbe7130d))

## [0.9.1](https://github.com/lostb1t/remux/compare/v0.9.0...v0.9.1) (2026-06-21)


### Bug Fixes

* pass --repo to gh release create to avoid missing git context ([99ad721](https://github.com/lostb1t/remux/commit/99ad721477228dbec84a45c8f75c83f9109632e5))
* query paramaters wrongly encoded resulting in wrong tmdb calls ([ef0ef77](https://github.com/lostb1t/remux/commit/ef0ef77a2b731cc2b07724875c59870295821cef))

# [0.9.0](https://github.com/lostb1t/remux/compare/v0.8.0...v0.9.0) (2026-06-21)


### Bug Fixes

* external subtitles for web ([aa335f1](https://github.com/lostb1t/remux/commit/aa335f10fa4817e827620fb847d76f2f18e0b904))
* force nextup active-series join order ([#26](https://github.com/lostb1t/remux/issues/26)) ([fefccdf](https://github.com/lostb1t/remux/commit/fefccdf7f6608513a47f44032eb42322a8e00c9c))
* handle progress reports without play session id ([#29](https://github.com/lostb1t/remux/issues/29)) ([5833a3f](https://github.com/lostb1t/remux/commit/5833a3f6679b7d0863cae8f57beea819fcb14539))
* inherit runtime from ([a7a53cb](https://github.com/lostb1t/remux/commit/a7a53cbf6b1ba401bf24b1f5b32d754f4eb3fa07))
* nextup was missing imported nedia [#14](https://github.com/lostb1t/remux/issues/14) ([5ec7471](https://github.com/lostb1t/remux/commit/5ec7471655db82523d1c37e1203423a6f23cd971))
* optimize iptv purge ([4c64a89](https://github.com/lostb1t/remux/commit/4c64a89d4ed50020252855585d79d2ab3999057e))
* order continue watching by play date ([#19](https://github.com/lostb1t/remux/issues/19)) ([17ac545](https://github.com/lostb1t/remux/commit/17ac5454fcb714df12a61b81819a8aef9e5d61a9))
* persist probe data between stream refresh ([f4212dd](https://github.com/lostb1t/remux/commit/f4212dd5b18696fcd72ac271566fd252642356ac))
* respect enable_user_data and normalize NextUp cutoff handling ([#21](https://github.com/lostb1t/remux/issues/21)) ([9270125](https://github.com/lostb1t/remux/commit/9270125e869a68b0c6f3f53de6a133a9e1b8350b))
* set DeliveryUrl on subtitle streams, respect device profile ([ac6c83b](https://github.com/lostb1t/remux/commit/ac6c83bb45698f63f65648b1adfce8c79c232ae4))
* use source bitrate as encoding target, cap at max_streaming_bitrate ([d17203e](https://github.com/lostb1t/remux/commit/d17203e5617f9381386f3045a2b098ee9e541f51))
* wrongly returning zero on items list with results ([d1125f1](https://github.com/lostb1t/remux/commit/d1125f16de8d32d8042ff9e8ddb707d6b96e0385))


### Features

* force plezy to reload versions ([4855d56](https://github.com/lostb1t/remux/commit/4855d560d0aed6ce9e9d2b734e15e3d7c9d1b2ab))
* Implement AudioLanguagePreference and RememberAudioSelections user settings ([46d1284](https://github.com/lostb1t/remux/commit/46d1284f6def125332707c58bd9bd035cbe7130d))

# [0.8.0](https://github.com/lostb1t/remux-server/compare/v0.7.0...v0.8.0) (2026-06-15)


### Bug Fixes

* missing channel guides ([e9bf908](https://github.com/lostb1t/remux-server/commit/e9bf9081aeac9865b1ee8aff2827bfa33ac47ca6))
* stream group lookup ([34ecf5e](https://github.com/lostb1t/remux-server/commit/34ecf5ec4595bd2b10c87f3d9a03c80e3a3de90c))


### Features

* downloads uses filename if avaiable ([1afc5f9](https://github.com/lostb1t/remux-server/commit/1afc5f93951e27d3eea35234503d492c47bdd258))

# [0.7.0](https://github.com/lostb1t/remux-server/compare/v0.6.0...v0.7.0) (2026-06-14)


### Bug Fixes

* enable download flag ([3ddf38d](https://github.com/lostb1t/remux-server/commit/3ddf38d1ac73bd89a5554117951c68ac6f078437))
* implement tree trait to tmdb addon ([a56c8ba](https://github.com/lostb1t/remux-server/commit/a56c8ba171cc630e0258a257faecc09b5817a356))
* make sure to load streams on audio endpoints ([0865b82](https://github.com/lostb1t/remux-server/commit/0865b8290076e75cef32384dc8b74cfa826cbbd1))


### Features

* add Jellyfin SDK-compatible user config route ([#89](https://github.com/lostb1t/remux-server/issues/89)) ([02414e9](https://github.com/lostb1t/remux-server/commit/02414e9ea35fb204030fbbc5acc4ef416ef25a93))
* implement /Items/{id}/Similar endpoint ([#87](https://github.com/lostb1t/remux-server/issues/87)) ([e765b3e](https://github.com/lostb1t/remux-server/commit/e765b3ee205d7feaf866ade8c418765de4bf333d))
* set default internet quality for jellyfin web to auto ([4c7bc9c](https://github.com/lostb1t/remux-server/commit/4c7bc9c88c5e1bf5d5e8558a44165fce9523932a))

# [0.6.0](https://github.com/lostb1t/remux-server/compare/v0.5.0...v0.6.0) (2026-06-11)


### Bug Fixes

* auth for jellyfin desktop ([0f644e6](https://github.com/lostb1t/remux-server/commit/0f644e670676bfbba0aac1491e7ee9fae4ff2414))


### Features

* add recommendations endpoints ([#83](https://github.com/lostb1t/remux-server/issues/83)) ([57b8226](https://github.com/lostb1t/remux-server/commit/57b82267e2665aeca263250dd5e08998206e0228))

# [0.5.0](https://github.com/lostb1t/remux-server/compare/v0.4.0...v0.5.0) (2026-06-10)


### Bug Fixes

* add music kinds to the media refresh task ([d62f2ea](https://github.com/lostb1t/remux-server/commit/d62f2ea3c5c6ed23049dbae30a8da294c81694ba))
* deezer track numbers ([133f099](https://github.com/lostb1t/remux-server/commit/133f099bc371dc054c84ce7cdcd490861f3a5eb7))
* deleted segments regardless of extension ([7052375](https://github.com/lostb1t/remux-server/commit/7052375a7f06c727c1aa9414985b7c89a52c872c))
* missing streams for local episodes ([722244f](https://github.com/lostb1t/remux-server/commit/722244fb206f0dafbd16e211b1da62f4f5a3e3be))
* music genres  ([#78](https://github.com/lostb1t/remux-server/issues/78)) ([eaa88ec](https://github.com/lostb1t/remux-server/commit/eaa88ecf1bd0b132214b1137ecbdd6aaae1e7d62))
* playlist crud ([7bb0d7b](https://github.com/lostb1t/remux-server/commit/7bb0d7b98f87d83333a12840567a69e211922b21))
* remove country code from parental rating ([df88ea6](https://github.com/lostb1t/remux-server/commit/df88ea62dd6cd89ec482c620e1ee93346e6c842c))


### Features

* add clear image cache task ([62834be](https://github.com/lostb1t/remux-server/commit/62834be14353c103fd08ed7a399ff58264e424fc))
* Add eclipse spotiFLAC and Monochrome addons ([#77](https://github.com/lostb1t/remux-server/issues/77)) ([2cf26b8](https://github.com/lostb1t/remux-server/commit/2cf26b8aeda27ea98b98fe164e4f321cc8b15688))
* Add option to disable video transcoding ([#76](https://github.com/lostb1t/remux-server/issues/76)) ([8ea1f71](https://github.com/lostb1t/remux-server/commit/8ea1f7166cfe0d6806c392ef727efe65644dc3d6))
* add sort and filter options for latest endpoints ([#75](https://github.com/lostb1t/remux-server/issues/75)) ([424e3b0](https://github.com/lostb1t/remux-server/commit/424e3b03e725939c6b1b33d0ad51e81e7f044774))
* add support for rtsp streams ([19013f7](https://github.com/lostb1t/remux-server/commit/19013f7fe703714159ad3afc402702e4654caff2))
* adding remote control endpoints and subtitle search endpoints ([#82](https://github.com/lostb1t/remux-server/issues/82)) ([8c31373](https://github.com/lostb1t/remux-server/commit/8c313734418aa69b4fd969fae18ecf1fcc0ed88b))
* import media during jellyfin favorites sync ([#79](https://github.com/lostb1t/remux-server/issues/79)) ([bf3d44b](https://github.com/lostb1t/remux-server/commit/bf3d44bea23405997d6e4e162a4dd15e12d889db))
* set sane homescreen defaults ([9acb85d](https://github.com/lostb1t/remux-server/commit/9acb85ddc1245ff4802c54362c5211f4c76aa081))
* support multiple paths in opendal addons ([6e6995e](https://github.com/lostb1t/remux-server/commit/6e6995ebb39bc51edc539375aea59babec8ec6d7))


### Performance Improvements

* add composite index on media_relations(left_media_id, weight) ([0aa7077](https://github.com/lostb1t/remux-server/commit/0aa70776a1acd7266348d555eeb5aece28169ed1))

# [0.4.0](https://github.com/lostb1t/remux-server/compare/v0.3.0...v0.4.0) (2026-05-30)


### Bug Fixes

* duplicate persons ([ad35109](https://github.com/lostb1t/remux-server/commit/ad35109491ffa9898eab56d63f4994626672e35d))
* fix corrupted external_ids case ([15a7e40](https://github.com/lostb1t/remux-server/commit/15a7e4023bc6dee6db48038c9ec27b39f88e098f))
* force h264 for encoding ([2619ead](https://github.com/lostb1t/remux-server/commit/2619eadf21c1b28e3fd3f693500627de73bd5897))
* libraries not showing when a user has filters ([4422031](https://github.com/lostb1t/remux-server/commit/44220316fe388a7fb20b5c132bf3a92d6093cd86))
* missing intro endpoint ([11cf16d](https://github.com/lostb1t/remux-server/commit/11cf16dfdfed9e67614fae707b9ae25d75a50377))
* nextup images ([3426268](https://github.com/lostb1t/remux-server/commit/342626865242c7a4c337912a3730d751fba14b05))
* people metadata ([80738ab](https://github.com/lostb1t/remux-server/commit/80738ab1952faa1a601e3a461e4179dd1bd5303d))
* scheduler not triggering ([2d00040](https://github.com/lostb1t/remux-server/commit/2d000401c36a40d6317bc95a42ffa04739a178a5))
* several EPG fixes ([42ce21c](https://github.com/lostb1t/remux-server/commit/42ce21cb14b3acc81cb5971ebc73fd6ce672faab))


### Features

* add clear cache task ([afcff08](https://github.com/lostb1t/remux-server/commit/afcff08512c25d1c5b03b2105ee38885b4414c1b))
* add Deezer SDK to remux-sdks ([ae90995](https://github.com/lostb1t/remux-server/commit/ae9099517fca0ea478b2dfac0ad1d72429b8f8a5))
* add max stream and remote search settings to user ([cdfeb90](https://github.com/lostb1t/remux-server/commit/cdfeb90b571f124ec55b5e7f715f73452dc558b8))
* extend user filters form ([95bbc5a](https://github.com/lostb1t/remux-server/commit/95bbc5a762b081cda1addf49fc7e67f14c196375))
* fallback to tmdb id if imdb does not resolve for stremio ([12c6ac4](https://github.com/lostb1t/remux-server/commit/12c6ac47cf63c289bdd08f2ce64febc48f6a5aa7))
* Mark parents played if all episodes are played and vice versa ([#71](https://github.com/lostb1t/remux-server/issues/71)) ([9e515d4](https://github.com/lostb1t/remux-server/commit/9e515d42ec195103d5311148dbc6df54357e93e9))
* per user stream filter ([718135b](https://github.com/lostb1t/remux-server/commit/718135bd449a0397e5534e414e8a9735f9b2f0d8))

# [0.3.0](https://github.com/lostb1t/remux-server/compare/v0.2.0...v0.3.0) (2026-05-19)


### Bug Fixes

* add vaapi docker packages and give qsv higher prio then vaapi ([1be17ab](https://github.com/lostb1t/remux-server/commit/1be17abbc96abb4992cd0cb02f9eb05faf9dbcd8))
* delete shows ([073bc76](https://github.com/lostb1t/remux-server/commit/073bc7670a88a45fd0ed5c490ce88a7b22e4aa80))
* docker hw packages ([ab7ad5c](https://github.com/lostb1t/remux-server/commit/ab7ad5c670dfe2680fc1d06957bb6e96cc94334c))
* external id field serialization ([c314013](https://github.com/lostb1t/remux-server/commit/c31401311d20c5b2fbd38936da1ebe4f196de31d))
* give catalog filter its onw field ([9331d3f](https://github.com/lostb1t/remux-server/commit/9331d3fa0f2c5e6fd088b377c5cf4daa3de687fb))
* hide catalog tags ([7a04651](https://github.com/lostb1t/remux-server/commit/7a04651e574c09929b8ba9833db9d90d048ef611))
* infuse fixes ([4c0cf2e](https://github.com/lostb1t/remux-server/commit/4c0cf2ee116c97433404db2c1073f67de265002a))
* loosen up digital release date filter ([5662a85](https://github.com/lostb1t/remux-server/commit/5662a859aeaf7263309e82e304d0762a52569935))
* missing enum variants ([336f0f4](https://github.com/lostb1t/remux-server/commit/336f0f485621bd6be7084b5e5637c86d7ecf344e))
* nissing migrations ([26fb94e](https://github.com/lostb1t/remux-server/commit/26fb94ecbdd43d3b7e40c9e3d66e8714ce8f8e7c))
* quickconnect ([3e541a7](https://github.com/lostb1t/remux-server/commit/3e541a7aaac6cf94575082ab12ff6a5c6bdc0205))
* report transcode info for remux sessions ([4b9d640](https://github.com/lostb1t/remux-server/commit/4b9d64094f91ce12d56ce1280a54572908b2cc83))
* wrong timestamps for date fields ([865a189](https://github.com/lostb1t/remux-server/commit/865a189f33f92362ec1889339e53b21e3c21afe9))


### Features

* add tonemapping packages for intel and more robust hw device detection ([62df8f7](https://github.com/lostb1t/remux-server/commit/62df8f7c2bab0d177769396db549e07291e4453d))
* HW acceleration ([#61](https://github.com/lostb1t/remux-server/issues/61)) ([fe7c0ac](https://github.com/lostb1t/remux-server/commit/fe7c0ac57b46096cb299cfb894a47944033ceb31))
* image support including avatars and auto generated collection images ([#62](https://github.com/lostb1t/remux-server/issues/62)) ([6bee985](https://github.com/lostb1t/remux-server/commit/6bee9854f50fedc1c2bab1b32045b35b4f8063cc))
* implement client log endpoint ([b884edc](https://github.com/lostb1t/remux-server/commit/b884edccd1431d31bc208fdff61a267488b227a1))
* stream fallback ([#63](https://github.com/lostb1t/remux-server/issues/63)) ([dd9c1ad](https://github.com/lostb1t/remux-server/commit/dd9c1ad225d9b97860942e641afa86ee54220e33))
* stream groups ([#64](https://github.com/lostb1t/remux-server/issues/64)) ([1854c4a](https://github.com/lostb1t/remux-server/commit/1854c4a36662e8b406f8dc5b10b02dd35a9dd6ed))
* user avatar support ([dbb76f2](https://github.com/lostb1t/remux-server/commit/dbb76f2b9125714ff5255af787b9fd63a52766e0))

# [0.2.0](https://github.com/lostb1t/remux-server/compare/v0.1.0...v0.2.0) (2026-05-10)


### Features

* add descriptions to tasks ([2f4f655](https://github.com/lostb1t/remux-server/commit/2f4f655bb10d41d96b431c40de4c29f533647431))
* clear addon indexes on purge ([1985249](https://github.com/lostb1t/remux-server/commit/19852492b120c0ded851bc4f8340c3a2a9f158ca))
* use proper parsing library for local files and support external id markers ([b24162f](https://github.com/lostb1t/remux-server/commit/b24162f5c5d4d591dadee0bac6ec2dc71e76f3f1))

# [0.1.0](https://github.com/lostb1t/remux-server/compare/v0.0.0...v0.1.0) (2026-05-10)


### Bug Fixes

* add default tmdb key ([501e6b8](https://github.com/lostb1t/remux-server/commit/501e6b8146cab947268f76c9da6da2df9c7793e5))
* add playback percentage to userdata ([6ef206a](https://github.com/lostb1t/remux-server/commit/6ef206abe5f2b245911acc3e08aaebb1b722cda7))
* always re-encode audio to AAC in HLS transcoding ([aa1444e](https://github.com/lostb1t/remux-server/commit/aa1444ed1fca5b8f41553ab039b124b3826a72a2))
* android tv playback ([df23949](https://github.com/lostb1t/remux-server/commit/df239496fcf77912761a55b6db4e9eaf26ebe276))
* client fixes ([#12](https://github.com/lostb1t/remux-server/issues/12)) ([3dea5ec](https://github.com/lostb1t/remux-server/commit/3dea5ec06ca3d31d469b89c6e2cb15e44625d4bc))
* fix optional fields ([f970df4](https://github.com/lostb1t/remux-server/commit/f970df4eb12f4c53ec6f626345e792407d696256))
* fix userdata not saving correctly and implement resume endpoints ([1c3daef](https://github.com/lostb1t/remux-server/commit/1c3daefb2f59337929c748e525a3a18db204a7f5))
* lower upsert chunk limit ([3437d92](https://github.com/lostb1t/remux-server/commit/3437d92c110dcbd516b189cd3267ee116b4552b0))
* revert item creation to 0.25 ([5278589](https://github.com/lostb1t/remux-server/commit/5278589e544acf604e96668d018759214eec13fa))
* test ([9c336d3](https://github.com/lostb1t/remux-server/commit/9c336d3b48af25e9e6653ad36c1a7212047591da))
* wip ([28cd9b2](https://github.com/lostb1t/remux-server/commit/28cd9b2a7eee3e3e9fa6d3a6ed663686578ffff7))
* wip ([fda6e10](https://github.com/lostb1t/remux-server/commit/fda6e1043b554ff19beebfaecbd4c303cdc6a44d))
* wip ([328107f](https://github.com/lostb1t/remux-server/commit/328107f0b057cf3e14b8bacb1fd126c26fb1cd2b))
* wip ([a506219](https://github.com/lostb1t/remux-server/commit/a50621975563d4092e50bbbbf93fe1bf57bbcb6c))
* wip ([df3ba0f](https://github.com/lostb1t/remux-server/commit/df3ba0f9d6fa19b9d5c102654e2e1c86c6d6e932))
* wip ([bf2e817](https://github.com/lostb1t/remux-server/commit/bf2e817d918c1ad35fdc8bba9870d5ce37376bcc))
* wip ([22a3f2a](https://github.com/lostb1t/remux-server/commit/22a3f2a95dc197006f3021fba5c80028790f8445))
* wip ([0a4604d](https://github.com/lostb1t/remux-server/commit/0a4604de83485b920bceb2b6a93c8c233aa304a6))
* wip ([b33d11d](https://github.com/lostb1t/remux-server/commit/b33d11de6c1aeeed460a97275fa1310acf54fc24))
* wip ([9e25d89](https://github.com/lostb1t/remux-server/commit/9e25d896d4cf7ac47e8c1742168b88f300bcd032))
* wip ([5b6f649](https://github.com/lostb1t/remux-server/commit/5b6f64945e4fe6c8d125583e1923f08b6c9632f8))
* wip ([6af5e1a](https://github.com/lostb1t/remux-server/commit/6af5e1abeac8a0a1baee75d4f46e1209309d40c2))
* wip ([abc00df](https://github.com/lostb1t/remux-server/commit/abc00dfe342209ae83befd04b2e504184b8b9cd2))
* wip ([2a5f986](https://github.com/lostb1t/remux-server/commit/2a5f986531d5f40f95ef7ab85ed1421a179eea07))
* wip ([88b116e](https://github.com/lostb1t/remux-server/commit/88b116ef3dee9f2b6681d69300da02fa3e99fc23))
* wip ([3919ec3](https://github.com/lostb1t/remux-server/commit/3919ec3bcfb0aa9c2c22e2072e4d47bedd632977))
* wip ([041fa19](https://github.com/lostb1t/remux-server/commit/041fa19cbdea0a2aeb04d6ca570675bd4e3568fa))
* wip ([a16e0eb](https://github.com/lostb1t/remux-server/commit/a16e0eb8a263c70c2251bd9a43e56579d8699399))
* wip ([db0f091](https://github.com/lostb1t/remux-server/commit/db0f091c93e5463c2bbc94849e9bcc945f0e35c2))
* wip ([c0673b7](https://github.com/lostb1t/remux-server/commit/c0673b714ce6debad0eb7300ad0d039924a9227e))
* wip ([0b35965](https://github.com/lostb1t/remux-server/commit/0b35965dcf91f087e21d251bd8f2bd98a1f9a354))


### Features

* add dual web-client flow and Anfiteatro release installer ([#24](https://github.com/lostb1t/remux-server/issues/24)) ([a7fea9a](https://github.com/lostb1t/remux-server/commit/a7fea9abc5c75f1087a3666b45764fdfae7e0219))
* migrate to FFmpeg-based probing and transcoding, fix seeking ([b261008](https://github.com/lostb1t/remux-server/commit/b261008e9ec6c2ca8fd2bc7b248c751e8a1bf578))
* Music ([#26](https://github.com/lostb1t/remux-server/issues/26)) ([2729992](https://github.com/lostb1t/remux-server/commit/2729992bd97ed9c799a308e72f8d4045ae81660d))
* seek ([#19](https://github.com/lostb1t/remux-server/issues/19)) ([59667be](https://github.com/lostb1t/remux-server/commit/59667bedd664284ffce6228b5dfcfaefb6e71bbf))

# 1.0.0 (2026-05-10)


### Bug Fixes

* add default tmdb key ([501e6b8](https://github.com/lostb1t/remux-server/commit/501e6b8146cab947268f76c9da6da2df9c7793e5))
* add playback percentage to userdata ([6ef206a](https://github.com/lostb1t/remux-server/commit/6ef206abe5f2b245911acc3e08aaebb1b722cda7))
* always re-encode audio to AAC in HLS transcoding ([aa1444e](https://github.com/lostb1t/remux-server/commit/aa1444ed1fca5b8f41553ab039b124b3826a72a2))
* android tv playback ([df23949](https://github.com/lostb1t/remux-server/commit/df239496fcf77912761a55b6db4e9eaf26ebe276))
* client fixes ([#12](https://github.com/lostb1t/remux-server/issues/12)) ([3dea5ec](https://github.com/lostb1t/remux-server/commit/3dea5ec06ca3d31d469b89c6e2cb15e44625d4bc))
* fix optional fields ([f970df4](https://github.com/lostb1t/remux-server/commit/f970df4eb12f4c53ec6f626345e792407d696256))
* fix userdata not saving correctly and implement resume endpoints ([1c3daef](https://github.com/lostb1t/remux-server/commit/1c3daefb2f59337929c748e525a3a18db204a7f5))
* lower upsert chunk limit ([3437d92](https://github.com/lostb1t/remux-server/commit/3437d92c110dcbd516b189cd3267ee116b4552b0))
* revert item creation to 0.25 ([5278589](https://github.com/lostb1t/remux-server/commit/5278589e544acf604e96668d018759214eec13fa))
* test ([9c336d3](https://github.com/lostb1t/remux-server/commit/9c336d3b48af25e9e6653ad36c1a7212047591da))
* wip ([28cd9b2](https://github.com/lostb1t/remux-server/commit/28cd9b2a7eee3e3e9fa6d3a6ed663686578ffff7))
* wip ([fda6e10](https://github.com/lostb1t/remux-server/commit/fda6e1043b554ff19beebfaecbd4c303cdc6a44d))
* wip ([328107f](https://github.com/lostb1t/remux-server/commit/328107f0b057cf3e14b8bacb1fd126c26fb1cd2b))
* wip ([a506219](https://github.com/lostb1t/remux-server/commit/a50621975563d4092e50bbbbf93fe1bf57bbcb6c))
* wip ([df3ba0f](https://github.com/lostb1t/remux-server/commit/df3ba0f9d6fa19b9d5c102654e2e1c86c6d6e932))
* wip ([bf2e817](https://github.com/lostb1t/remux-server/commit/bf2e817d918c1ad35fdc8bba9870d5ce37376bcc))
* wip ([22a3f2a](https://github.com/lostb1t/remux-server/commit/22a3f2a95dc197006f3021fba5c80028790f8445))
* wip ([0a4604d](https://github.com/lostb1t/remux-server/commit/0a4604de83485b920bceb2b6a93c8c233aa304a6))
* wip ([b33d11d](https://github.com/lostb1t/remux-server/commit/b33d11de6c1aeeed460a97275fa1310acf54fc24))
* wip ([9e25d89](https://github.com/lostb1t/remux-server/commit/9e25d896d4cf7ac47e8c1742168b88f300bcd032))
* wip ([5b6f649](https://github.com/lostb1t/remux-server/commit/5b6f64945e4fe6c8d125583e1923f08b6c9632f8))
* wip ([6af5e1a](https://github.com/lostb1t/remux-server/commit/6af5e1abeac8a0a1baee75d4f46e1209309d40c2))
* wip ([abc00df](https://github.com/lostb1t/remux-server/commit/abc00dfe342209ae83befd04b2e504184b8b9cd2))
* wip ([2a5f986](https://github.com/lostb1t/remux-server/commit/2a5f986531d5f40f95ef7ab85ed1421a179eea07))
* wip ([88b116e](https://github.com/lostb1t/remux-server/commit/88b116ef3dee9f2b6681d69300da02fa3e99fc23))
* wip ([3919ec3](https://github.com/lostb1t/remux-server/commit/3919ec3bcfb0aa9c2c22e2072e4d47bedd632977))
* wip ([041fa19](https://github.com/lostb1t/remux-server/commit/041fa19cbdea0a2aeb04d6ca570675bd4e3568fa))
* wip ([a16e0eb](https://github.com/lostb1t/remux-server/commit/a16e0eb8a263c70c2251bd9a43e56579d8699399))
* wip ([db0f091](https://github.com/lostb1t/remux-server/commit/db0f091c93e5463c2bbc94849e9bcc945f0e35c2))
* wip ([c0673b7](https://github.com/lostb1t/remux-server/commit/c0673b714ce6debad0eb7300ad0d039924a9227e))
* wip ([0b35965](https://github.com/lostb1t/remux-server/commit/0b35965dcf91f087e21d251bd8f2bd98a1f9a354))


### Features

* add dual web-client flow and Anfiteatro release installer ([#24](https://github.com/lostb1t/remux-server/issues/24)) ([a7fea9a](https://github.com/lostb1t/remux-server/commit/a7fea9abc5c75f1087a3666b45764fdfae7e0219))
* migrate to FFmpeg-based probing and transcoding, fix seeking ([b261008](https://github.com/lostb1t/remux-server/commit/b261008e9ec6c2ca8fd2bc7b248c751e8a1bf578))
* Music ([#26](https://github.com/lostb1t/remux-server/issues/26)) ([2729992](https://github.com/lostb1t/remux-server/commit/2729992bd97ed9c799a308e72f8d4045ae81660d))
* seek ([#19](https://github.com/lostb1t/remux-server/issues/19)) ([59667be](https://github.com/lostb1t/remux-server/commit/59667bedd664284ffce6228b5dfcfaefb6e71bbf))

# 1.0.0 (2026-03-27)

### Bug Fixes

* always re-encode audio to AAC in HLS transcoding ([aa1444e](https://github.com/Remuxd/remux-server/commit/aa1444ed1fca5b8f41553ab039b124b3826a72a2))
* revert item creation to 0.25 ([5278589](https://github.com/Remuxd/remux-server/commit/5278589e544acf604e96668d018759214eec13fa))
* wip ([3919ec3](https://github.com/Remuxd/remux-server/commit/3919ec3bcfb0aa9c2c22e2072e4d47bedd632977))
* wip ([041fa19](https://github.com/Remuxd/remux-server/commit/041fa19cbdea0a2aeb04d6ca570675bd4e3568fa))
* wip ([a16e0eb](https://github.com/Remuxd/remux-server/commit/a16e0eb8a263c70c2251bd9a43e56579d8699399))
* wip ([db0f091](https://github.com/Remuxd/remux-server/commit/db0f091c93e5463c2bbc94849e9bcc945f0e35c2))
* wip ([c0673b7](https://github.com/Remuxd/remux-server/commit/c0673b714ce6debad0eb7300ad0d039924a9227e))
* wip ([0b35965](https://github.com/Remuxd/remux-server/commit/0b35965dcf91f087e21d251bd8f2bd98a1f9a354))


### Features

* migrate to FFmpeg-based probing and transcoding, fix seeking ([b261008](https://github.com/Remuxd/remux-server/commit/b261008e9ec6c2ca8fd2bc7b248c751e8a1bf578))

# 1.0.0 (2026-03-27)


### Bug Fixes

* always re-encode audio to AAC in HLS transcoding ([aa1444e](https://github.com/Remuxd/remux-server/commit/aa1444ed1fca5b8f41553ab039b124b3826a72a2))
* revert item creation to 0.25 ([5278589](https://github.com/Remuxd/remux-server/commit/5278589e544acf604e96668d018759214eec13fa))
* wip ([3919ec3](https://github.com/Remuxd/remux-server/commit/3919ec3bcfb0aa9c2c22e2072e4d47bedd632977))
* wip ([041fa19](https://github.com/Remuxd/remux-server/commit/041fa19cbdea0a2aeb04d6ca570675bd4e3568fa))
* wip ([a16e0eb](https://github.com/Remuxd/remux-server/commit/a16e0eb8a263c70c2251bd9a43e56579d8699399))
* wip ([db0f091](https://github.com/Remuxd/remux-server/commit/db0f091c93e5463c2bbc94849e9bcc945f0e35c2))
* wip ([c0673b7](https://github.com/Remuxd/remux-server/commit/c0673b714ce6debad0eb7300ad0d039924a9227e))
* wip ([0b35965](https://github.com/Remuxd/remux-server/commit/0b35965dcf91f087e21d251bd8f2bd98a1f9a354))


### Features

* migrate to FFmpeg-based probing and transcoding, fix seeking ([b261008](https://github.com/Remuxd/remux-server/commit/b261008e9ec6c2ca8fd2bc7b248c751e8a1bf578))
