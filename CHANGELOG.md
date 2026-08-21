# [0.26.0](https://github.com/lostb1t/remux/compare/v0.25.0...v0.26.0) (2026-08-21)


### Bug Fixes

* **catalog:** import catalogs regardless of declared type, filter per item (fixes [#267](https://github.com/lostb1t/remux/issues/267)) ([260f05f](https://github.com/lostb1t/remux/commit/260f05fd135fe943cd014b5bc2fea7a835d6bc79))
* **catalog:** remove stale catalog members that drop out of a re-import ([8199419](https://github.com/lostb1t/remux/commit/8199419c5aca6f5a6cf6e4762c3d5a210782470c))
* **collections:** stop deleting primary image on every settings save (fixes [#285](https://github.com/lostb1t/remux/issues/285)) ([81ee0ff](https://github.com/lostb1t/remux/commit/81ee0ff6e92dfd17b915854b5a8f1608477ccd17))
* don't delete Xcode.app in macOS disk cleanup — breaks codesign ([eaf7af0](https://github.com/lostb1t/remux/commit/eaf7af00dd5d4202e50684b74750dcdd2a6b488b))
* **episodes:** remove incorrect backdrop fallback that broke episode backdrop display ([33b5c81](https://github.com/lostb1t/remux/commit/33b5c813aa3280b4a397c46b40db9e507eceeba6))
* honour enable_video_transcoding on SupportsTranscoding and transcode decisions ([#283](https://github.com/lostb1t/remux/issues/283)) ([b33f49e](https://github.com/lostb1t/remux/commit/b33f49e61313c9e02bb160be474f91d031dae8e0))
* honour the selected stream group on Android TV ([#239](https://github.com/lostb1t/remux/issues/239)) ([44b2d58](https://github.com/lostb1t/remux/commit/44b2d5836594caab374ef6398a409798ce440b30))
* **images:** regenerate collection image and bust cache on delete (fixes [#209](https://github.com/lostb1t/remux/issues/209)) ([abb6a3f](https://github.com/lostb1t/remux/commit/abb6a3f37253689aa46a2d3dc2ce321eb564012b))
* **items:** restore episode backdrop fallback ([#271](https://github.com/lostb1t/remux/issues/271)) ([dd1e23c](https://github.com/lostb1t/remux/commit/dd1e23c7a591a0e6b1800786e2937fc790a97e91))
* **jellyfin:** align server routes and playback requests ([#257](https://github.com/lostb1t/remux/issues/257)) ([9dadeff](https://github.com/lostb1t/remux/commit/9dadeff4ae53696e42d2fab2302b10c33a4be140))
* **jellyfin:** preserve session message protocol shapes ([#273](https://github.com/lostb1t/remux/issues/273)) ([bcd7703](https://github.com/lostb1t/remux/commit/bcd770359ad50b804210b1c15e6c4ac608ba26c7))
* **livetv:** fix EPG not showing in guide (fixes [#223](https://github.com/lostb1t/remux/issues/223)) ([bc9af33](https://github.com/lostb1t/remux/commit/bc9af331692beb311a4d5b8295a2a6227b220a56))
* **macos:** detect VideoToolbox AV1 decode support ([#259](https://github.com/lostb1t/remux/issues/259)) ([04bdfa5](https://github.com/lostb1t/remux/commit/04bdfa5aa660ffd26964e0bc9f9e6c558ff8a83e))
* **music:** show singles/EPs in Albums and artist pages again (fixes [#208](https://github.com/lostb1t/remux/issues/208)) ([6a6de8f](https://github.com/lostb1t/remux/commit/6a6de8fab2082f20bd7198bc9e5d2ba91fcad70b)), closes [#178](https://github.com/lostb1t/remux/issues/178)
* **nextup:** fix limit placement, EnableResumable=false candidate check, and start_index paging ([6f4add7](https://github.com/lostb1t/remux/commit/6f4add71831d1788fc409cd273aeb1c4a5aa43ef))
* pass through unknown quality/codec in stream filter; 404 on subtitle lookup miss ([#232](https://github.com/lostb1t/remux/issues/232)) ([3e1ec99](https://github.com/lostb1t/remux/commit/3e1ec99c7edb599f93f908983bb8f772a210db9f))
* **playback:** notify clients after playback stops ([#281](https://github.com/lostb1t/remux/issues/281)) ([fd13105](https://github.com/lostb1t/remux/commit/fd13105333b151581b4b2524e4d58b579b7079d3))
* **playback:** preserve item duration for unprobed streams ([#274](https://github.com/lostb1t/remux/issues/274)) ([5ba5d31](https://github.com/lostb1t/remux/commit/5ba5d31841b675b6dd0572b23c16d4fc697bed61))
* populate Id and expand field mapping for /Search/Hints ([#231](https://github.com/lostb1t/remux/issues/231)) ([cf10056](https://github.com/lostb1t/remux/commit/cf10056a29b4fac8012e94edcb105e62ed0b5cb7))
* redact secrets from Debug output ([#238](https://github.com/lostb1t/remux/issues/238)) ([035b421](https://github.com/lostb1t/remux/commit/035b42121aeb36df54a95d564ab59e007a2b128b))
* **runtime:** raise the Unix open-file soft limit ([#258](https://github.com/lostb1t/remux/issues/258)) ([1832d07](https://github.com/lostb1t/remux/commit/1832d076a45c3c17874c3fe5a0140a9f3716c85b))
* stable stream dedup key using filename/size/bingeGroup/serviceId/addonId (fixes [#246](https://github.com/lostb1t/remux/issues/246)) ([0b6c623](https://github.com/lostb1t/remux/commit/0b6c62314ccf9bfc8a6402774bed00051813e5c2))
* **stremio:** preserve nested torrent metadata ([#275](https://github.com/lostb1t/remux/issues/275)) ([443a3b2](https://github.com/lostb1t/remux/commit/443a3b259c2adcb071bf77fe694eb566686a4862))
* **subtitles:** don't 500 when an external subtitle CDN fetch fails ([899477e](https://github.com/lostb1t/remux/commit/899477ec5af94779457f507b708acce5c7ab16b3))
* **subtitles:** handle CRLF cue boundaries ([#280](https://github.com/lostb1t/remux/issues/280)) ([0b2c92b](https://github.com/lostb1t/remux/commit/0b2c92b983c9d516c0daac3ca00ec9ffa6c42eef))
* **torrent:** stream only the selected bundle file ([#277](https://github.com/lostb1t/remux/issues/277)) ([a0bafa9](https://github.com/lostb1t/remux/commit/a0bafa9f150ae98075c629b86681db4ccf992d50))
* **tracking:** an undeliverable item is not a broken connection ([#279](https://github.com/lostb1t/remux/issues/279)) ([062aff1](https://github.com/lostb1t/remux/commit/062aff1a7d3dd6a1c1acd1cd8c3a709e8dc9bde0))
* **transcode:** fall back to writable temporary storage ([#276](https://github.com/lostb1t/remux/issues/276)) ([1118782](https://github.com/lostb1t/remux/commit/1118782f657576d3303ec78d8b995ff68c9944d3))
* **users:** honour the target user on favourite endpoints ([#236](https://github.com/lostb1t/remux/issues/236)) ([3bf413f](https://github.com/lostb1t/remux/commit/3bf413f2ddee2597c6a3964edb66effa1b3d3022))
* **windows:** spawn ffmpeg/ffprobe/yt-dlp without console windows (CREATE_NO_WINDOW) ([f822be0](https://github.com/lostb1t/remux/commit/f822be0dadc0ba8746e5773de215c1e4f67e4745))


### Features

* **addons:** add a common interface for tracking addons ([#222](https://github.com/lostb1t/remux/issues/222)) ([f84303a](https://github.com/lostb1t/remux/commit/f84303abd871014768a8f74840ba1c3d9b190081))
* **db:** add user_media_trackers for per-user media tracking connections ([#240](https://github.com/lostb1t/remux/issues/240)) ([309405b](https://github.com/lostb1t/remux/commit/309405b7753baeb00154e0b7ad0075c0b48af6e8))
* make watch thresholds configurable (fixes [#211](https://github.com/lostb1t/remux/issues/211)) ([#225](https://github.com/lostb1t/remux/issues/225)) ([380067b](https://github.com/lostb1t/remux/commit/380067ba50e139ea2d961fe634340c9a562b821e))
* **torrent:** pass addon-provided trackers through to the magnet ([#266](https://github.com/lostb1t/remux/issues/266)) ([4f5f9df](https://github.com/lostb1t/remux/commit/4f5f9df2427fd2f0c36f999fd76d9079386bcb62))
* **tracking:** add the delivery queue and its retry worker ([#242](https://github.com/lostb1t/remux/issues/242)) ([46df919](https://github.com/lostb1t/remux/commit/46df91911c2bd0c8db7bb77feff9377f6d1ffb56))

# [0.25.0](https://github.com/lostb1t/remux/compare/v0.24.3...v0.25.0) (2026-08-15)


### Bug Fixes

* carry probe_data streams in no-streams stub and skip streams_refreshed_at when refresh yields nothing ([4483821](https://github.com/lostb1t/remux/commit/4483821c81ecd18c233657f64da61658b10bbd77))
* delete catalog collection_kind rows and ignore missing migrations ([41bfdbd](https://github.com/lostb1t/remux/commit/41bfdbd23018e324611b7df50105912a9207a437))
* load grandparent before subtitle addon lookup for episodes ([ba6f152](https://github.com/lostb1t/remux/commit/ba6f1524832b4d053391de4ce79397bfe832d536))
* never serve an empty fMP4 init segment (HEVC 10-bit browser playback) ([#228](https://github.com/lostb1t/remux/issues/228)) ([fc9242f](https://github.com/lostb1t/remux/commit/fc9242f4782f440d2f5ec6f3359ef2dd14c9ad9e))
* redirect transcode and DHT cache paths under data_dir for non-root Docker (fixes [#200](https://github.com/lostb1t/remux/issues/200)) ([caa2def](https://github.com/lostb1t/remux/commit/caa2defcc8d63b4c610629659332e4f23cf580b4))
* replace delete-catalog migration with update to smart ([1949908](https://github.com/lostb1t/remux/commit/1949908d1dc4f506e5d4af89da2206bbf59d1714))
* update jellyfin-ffmpeg asset suffixes and add tar.xz extraction support ([3b95da5](https://github.com/lostb1t/remux/commit/3b95da56ca393332b370fef7d30f4a22c298aa55))


### Features

* add http_redirect_stream and service_filter per addon ([#233](https://github.com/lostb1t/remux/issues/233)) ([1319b94](https://github.com/lostb1t/remux/commit/1319b949db7d9050978343908fcc9a2119b40e24))
* add jellyfin_version config option (fixes [#221](https://github.com/lostb1t/remux/issues/221)) ([949465f](https://github.com/lostb1t/remux/commit/949465f39dadc8250fa94e5fad1a70cab7d8549a))
* return no-streams stub and video when item has no playable sources (fix [#212](https://github.com/lostb1t/remux/issues/212)) ([#219](https://github.com/lostb1t/remux/issues/219)) ([aa3c02f](https://github.com/lostb1t/remux/commit/aa3c02ff27ec9df3164317771f7f9b6ac5d771c2))
* **users:** add Jellyfin personal rating endpoints ([#224](https://github.com/lostb1t/remux/issues/224)) ([817bce8](https://github.com/lostb1t/remux/commit/817bce8957db754da041b74013b43929c1b321e1))

## [0.25.1](https://github.com/lostb1t/remux/compare/v0.25.0...v0.25.1) (2026-08-15)


### Bug Fixes

* delete catalog collection_kind rows and ignore missing migrations ([41bfdbd](https://github.com/lostb1t/remux/commit/41bfdbd23018e324611b7df50105912a9207a437))

# [0.25.0](https://github.com/lostb1t/remux/compare/v0.24.3...v0.25.0) (2026-08-15)


### Bug Fixes

* carry probe_data streams in no-streams stub and skip streams_refreshed_at when refresh yields nothing ([4483821](https://github.com/lostb1t/remux/commit/4483821c81ecd18c233657f64da61658b10bbd77))
* load grandparent before subtitle addon lookup for episodes ([ba6f152](https://github.com/lostb1t/remux/commit/ba6f1524832b4d053391de4ce79397bfe832d536))
* never serve an empty fMP4 init segment (HEVC 10-bit browser playback) ([#228](https://github.com/lostb1t/remux/issues/228)) ([fc9242f](https://github.com/lostb1t/remux/commit/fc9242f4782f440d2f5ec6f3359ef2dd14c9ad9e))
* redirect transcode and DHT cache paths under data_dir for non-root Docker (fixes [#200](https://github.com/lostb1t/remux/issues/200)) ([caa2def](https://github.com/lostb1t/remux/commit/caa2defcc8d63b4c610629659332e4f23cf580b4))
* update jellyfin-ffmpeg asset suffixes and add tar.xz extraction support ([3b95da5](https://github.com/lostb1t/remux/commit/3b95da56ca393332b370fef7d30f4a22c298aa55))


### Features

* add jellyfin_version config option (fixes [#221](https://github.com/lostb1t/remux/issues/221)) ([949465f](https://github.com/lostb1t/remux/commit/949465f39dadc8250fa94e5fad1a70cab7d8549a))
* return no-streams stub and video when item has no playable sources (fix [#212](https://github.com/lostb1t/remux/issues/212)) ([#219](https://github.com/lostb1t/remux/issues/219)) ([aa3c02f](https://github.com/lostb1t/remux/commit/aa3c02ff27ec9df3164317771f7f9b6ac5d771c2))
* **users:** add Jellyfin personal rating endpoints ([#224](https://github.com/lostb1t/remux/issues/224)) ([817bce8](https://github.com/lostb1t/remux/commit/817bce8957db754da041b74013b43929c1b321e1))

## [0.24.3](https://github.com/lostb1t/remux/compare/v0.24.2...v0.24.3) (2026-08-12)


### Bug Fixes

* serialize UUIDs without hyphens in all DTO responses for Infuse compatibility ([e293a58](https://github.com/lostb1t/remux/commit/e293a5828679d872a49d5ae808a26c1de12c98d5))
* update test assertions to expect no-hyphen UUID format in API responses ([1f260f2](https://github.com/lostb1t/remux/commit/1f260f2d7b86a5cd4c4a3630a1e44c9f74fe3974))

## [0.24.2](https://github.com/lostb1t/remux/compare/v0.24.1...v0.24.2) (2026-08-11)


### Bug Fixes

* preserve catalog stream order when assigning weights to new items ([96fe01e](https://github.com/lostb1t/remux/commit/96fe01ee1faa28a7fda04792f1bb78cddd2af146))

## [0.24.1](https://github.com/lostb1t/remux/compare/v0.24.0...v0.24.1) (2026-08-10)


### Bug Fixes

* URL-decode path before embedded file lookup so %40 (@) filenames resolve ([544f7a9](https://github.com/lostb1t/remux/commit/544f7a99648e3737686ef74eda378b249c698f99))

# [0.24.0](https://github.com/lostb1t/remux/compare/v0.23.1...v0.24.0) (2026-08-10)


### Bug Fixes

* accept any ext-id UUID candidate in media validate to fix [❌] stub collisions ([648d8c6](https://github.com/lostb1t/remux/commit/648d8c6cd658ed3016f8405f1d42c1489972e0d1))
* downgrade fetch_subtitles log to debug ([1ae2e42](https://github.com/lostb1t/remux/commit/1ae2e4266aa451a97f66e35c0f8123a53574facc))
* drive desktop asset embedding via DASHBOARD_PATH/WEB_PATH env vars ([2dc8618](https://github.com/lostb1t/remux/commit/2dc86183b5ff3d68b3babaec8199349d49ca66fe))
* external-ID deduplication and tree-aware import ([#165](https://github.com/lostb1t/remux/issues/165)) ([a75295d](https://github.com/lostb1t/remux/commit/a75295d870e6341a9671e8009ac964c9bea01dcc))
* free disk space before desktop build on macOS runners ([692c44e](https://github.com/lostb1t/remux/commit/692c44e509bba2c33eb875f881d4eb2c717547cb))
* lazy subtitle extraction — only extract when subtitle URL is requested ([1cb906a](https://github.com/lostb1t/remux/commit/1cb906ab1e854158883548f2e1f3ba312fed405e))
* log fetch_subtitles at info only for real fetches, debug for cached lookups ([266af94](https://github.com/lostb1t/remux/commit/266af94690cdd71f970d270f349ffaba240eb15d))
* **playlists:** return all media kinds from playlist items endpoint ([#199](https://github.com/lostb1t/remux/issues/199)) ([ca67af4](https://github.com/lostb1t/remux/commit/ca67af4ca0267cb06c2910ec8b238013b6be5733))
* restore portable jellyfin-ffmpeg asset filter and add system ffmpeg fallback ([6d34317](https://github.com/lostb1t/remux/commit/6d3431741df6007b64687c4b59cb3c4009ea9458))
* route librqbit and yt-dlp cache writes to data_dir/cache (fixes [#200](https://github.com/lostb1t/remux/issues/200)) ([f01d3e6](https://github.com/lostb1t/remux/commit/f01d3e6754592c91f6529036e24c549173de1a89))
* skip remuxdb submission when stream has no torrent hash or nzb (fixes [#202](https://github.com/lostb1t/remux/issues/202)) ([d795afa](https://github.com/lostb1t/remux/commit/d795afaac3c8749715a2226d43aa7ac213500be1))
* skip stremio error stubs in catalog stream and search ([2340312](https://github.com/lostb1t/remux/commit/2340312797aa3e098204f8dde248eaf5245b415b))
* strip manifest.json from StremioManifestUrl when query string is present (fixes [#203](https://github.com/lostb1t/remux/issues/203)) ([48377ee](https://github.com/lostb1t/remux/commit/48377eea5edce213a690e4dcbaf5fed8fab15d1b))
* use absolute paths for DASHBOARD_PATH/WEB_PATH in desktop CI build ([b1da7be](https://github.com/lostb1t/remux/commit/b1da7be401da474251d8e25e9fe23aeb61118c09))
* **web:** re-apply real track selections when core re-renders after playback ([408928d](https://github.com/lostb1t/remux/commit/408928d13119ce498fdb2e4c487ed133e67f1f15))


### Features

* add default ElegantFin CSS to BrandingOptions ([d66c92d](https://github.com/lostb1t/remux/commit/d66c92dbe87234e37055a963673eef30ce0f5e09))
* allow JS injection on branding page (fixes [#190](https://github.com/lostb1t/remux/issues/190)) ([8e297d8](https://github.com/lostb1t/remux/commit/8e297d818a8f95cbbd2c24c30acffb03ea093db4))
* forward manifest URL query params to all addon resource requests (fixes [#203](https://github.com/lostb1t/remux/issues/203)) ([7657629](https://github.com/lostb1t/remux/commit/765762953f968d2d19e7cecf1eb6adc8ed6b5beb))
* idPrefixes-aware addon routing and stream ID selection ([#193](https://github.com/lostb1t/remux/issues/193)) ([e0638bf](https://github.com/lostb1t/remux/commit/e0638bfb410a55e17ab84db8144be98b9549c220))
* implement PlayDefaultAudioTrack using original_language from DB ([7f991ab](https://github.com/lostb1t/remux/commit/7f991ab64426c607bceda50bd438e0d1f8d4b462))
* nested collection browsing with smart group containers ([#205](https://github.com/lostb1t/remux/issues/205)) ([44af3a8](https://github.com/lostb1t/remux/commit/44af3a8951cc4bf560e73a2d4000038bb97ff96a))
* **sessions:** admin session revocation and activity log ([#128](https://github.com/lostb1t/remux/issues/128)) ([15e4197](https://github.com/lostb1t/remux/commit/15e4197eb00a3660d2d72ac77ddfaf482330ed65))
* support animated GIF images; preserve image format extension on save ([7b82965](https://github.com/lostb1t/remux/commit/7b82965650417211618167e55723ad111d32e49e))

## [0.23.1](https://github.com/lostb1t/remux/compare/v0.23.0...v0.23.1) (2026-08-05)


### Bug Fixes

* only override collection default sort when SortName is the primary key ([ffacd4b](https://github.com/lostb1t/remux/commit/ffacd4b3eefb0226de6306aa7814288aee89af5b))

# [0.23.0](https://github.com/lostb1t/remux/compare/v0.22.0...v0.23.0) (2026-08-05)


### Bug Fixes

* clear the MediaTypes query param ([#187](https://github.com/lostb1t/remux/issues/187)) ([00da83e](https://github.com/lostb1t/remux/commit/00da83ee80f3002d0d4e724e0b04a5fc3bcf936d))
* derive default audio/subtitle stream indexes per request ([#176](https://github.com/lostb1t/remux/issues/176)) ([1876100](https://github.com/lostb1t/remux/commit/18761006d32ad2fe08c2215b605460d8c8166419))
* **desktop:** decouple dashboard and web-client embeds ([b6e18bd](https://github.com/lostb1t/remux/commit/b6e18bd58158b32ece116a0712075a08d1089b36))
* emit two stub media sources in listings, route resume through items query ([9367ff6](https://github.com/lostb1t/remux/commit/9367ff6cb97a45498e77fa74c1df10cb4bb65d87))
* global metadata language fallback for subtitle selection ([#174](https://github.com/lostb1t/remux/issues/174)) ([9c8e489](https://github.com/lostb1t/remux/commit/9c8e489654a093f4d5a012ce668543ba03128946))
* **music:** use flat artist/album names for playlist imports ([#170](https://github.com/lostb1t/remux/issues/170)) ([1b6b385](https://github.com/lostb1t/remux/commit/1b6b3855f808aa55a132731f77d21baf29f3b0b2))
* **subtitles:** serve native ASS from a separate cache ([#180](https://github.com/lostb1t/remux/issues/180)) ([158cebe](https://github.com/lostb1t/remux/commit/158cebebb2ebc9abe57796604a355e89a650cbd8))
* **subtitles:** support tickless subtitle route ([#183](https://github.com/lostb1t/remux/issues/183)) ([c38019e](https://github.com/lostb1t/remux/commit/c38019e9fb36bbc785eb8a4e1c5ec16415ac06aa))
* **web:** keep track panel hidden during stream load, render spinner inside it ([c1b23eb](https://github.com/lostb1t/remux/commit/c1b23ebf263895e90288e7df38094af00cf55153))


### Features

* **items:** implement missing sort arms ([#177](https://github.com/lostb1t/remux/issues/177)) ([7ec6c76](https://github.com/lostb1t/remux/commit/7ec6c76540ab279ed4fa527429f916ba9bba1ee0))
* **music:** exclude singles/EPs from albums section ([#178](https://github.com/lostb1t/remux/issues/178)) ([1252b8f](https://github.com/lostb1t/remux/commit/1252b8ff2b934255137be1fd87e55fd0ffeb68a7))


### Performance Improvements

* pool the stream proxy HTTP client for keep-alive reuse ([#181](https://github.com/lostb1t/remux/issues/181)) ([3603a1e](https://github.com/lostb1t/remux/commit/3603a1e8c707e92a2ba25ab39eb7f19d84813b5f))

# [0.22.0](https://github.com/lostb1t/remux/compare/v0.21.0...v0.22.0) (2026-08-04)


### Bug Fixes

* correct gregorian day constant in dashboard uptime parser ([#168](https://github.com/lostb1t/remux/issues/168)) ([87d8305](https://github.com/lostb1t/remux/commit/87d8305e224e1159f8542cf22f85db24d1524217))
* fall back to container bitrate for video_bitrate when stream level is absent, hide uri from log spans ([eee8558](https://github.com/lostb1t/remux/commit/eee8558f1f8b2eb235a4ed0d2fac4d4148bd2f4e))
* include Audio, MusicAlbum and MusicArtist in default search types ([9bccaf7](https://github.com/lostb1t/remux/commit/9bccaf7d7c83a28920b9da23d95d1b0f0dafe7e8)), closes [#157](https://github.com/lostb1t/remux/issues/157)
* match jellyfin-ffmpeg assets by platform suffix and extension only ([62541fb](https://github.com/lostb1t/remux/commit/62541fbef6102e12d2d26f85d4529d91d8efbf5d))
* move codec enums to remux-sdks, add typed accessors, fix IsTextSubtitleStream for text subtitle streams ([937b5bd](https://github.com/lostb1t/remux/commit/937b5bd996818c4950bf12909786b5bc73032fd4))
* re-key legacy user_media_state to current media UUID on IMDB content fallback ([607a338](https://github.com/lostb1t/remux/commit/607a338443c015ce10f10524ee0f373214d3afe4))
* serve synthetic VOD playlist for fMP4 sessions to enable full seek bar, bump TARGETDURATION ceiling ([b13cff5](https://github.com/lostb1t/remux/commit/b13cff5d51bf21a29e24f27fb7174a923ab0609d))
* server-side admin gate for dashboard ([#162](https://github.com/lostb1t/remux/issues/162)) ([0c00338](https://github.com/lostb1t/remux/commit/0c00338b2293145baa4db09680c3f3bec2093eba))
* show artist and album name for deezer playlist tracks ([#163](https://github.com/lostb1t/remux/issues/163)) ([#164](https://github.com/lostb1t/remux/issues/164)) ([7425e8d](https://github.com/lostb1t/remux/commit/7425e8d875a1309ca6dfcfaa4c919ce837119cd3))
* strip Windows UNC prefix from embedded asset paths in build.rs ([1f4346e](https://github.com/lostb1t/remux/commit/1f4346ee03bbeffc34470c02d0aa88152ad45258))
* suppress console window on Windows ([f17b6a5](https://github.com/lostb1t/remux/commit/f17b6a521b3815bdc0b24f2c35630bcfed2b9424))
* SW decode + tonemapx for VideoToolbox HDR transcodes ([574952a](https://github.com/lostb1t/remux/commit/574952a487eb570d01d573e1352c0cbde25673e3))
* use TargetUser for GET /items/{id}, omit zero PlayedPercentage, fix Key field ([3af3bf0](https://github.com/lostb1t/remux/commit/3af3bf0dcb60a60cd2a84b267785b75abfcc107f))


### Features

* apply subtitle/audio language preference defaults in Items endpoint ([692ead7](https://github.com/lostb1t/remux/commit/692ead76f0c04c6cf3c877f89f79a83a7d500aea))

# [0.21.0](https://github.com/lostb1t/remux/compare/v0.20.2...v0.21.0) (2026-08-02)


### Bug Fixes

* ad-hoc codesign macOS app bundle before DMG packaging ([579270f](https://github.com/lostb1t/remux/commit/579270ffa2cfe0022d7eb4e014723f1c8f3eb0c3))


### Features

* add healthcheck endpoint ([#155](https://github.com/lostb1t/remux/issues/155)) ([e960772](https://github.com/lostb1t/remux/commit/e960772e4193e575d3dcc99262253577974a2a1e))

## [0.20.2](https://github.com/lostb1t/remux/compare/v0.20.1...v0.20.2) (2026-08-02)


### Bug Fixes

* restore WEB_PATH and DASHBOARD_PATH env var support for filesystem paths ([fa04cb6](https://github.com/lostb1t/remux/commit/fa04cb6f9ac2b6d01241da4cffe52ac3a9d99f0e))

## [0.20.1](https://github.com/lostb1t/remux/compare/v0.20.0...v0.20.1) (2026-08-02)


### Bug Fixes

* upload desktop artifacts with stable names for latest/download links ([a940bad](https://github.com/lostb1t/remux/commit/a940badcbe5b7954c83b7c81f21cd60167a783f3))

# [0.20.0](https://github.com/lostb1t/remux/compare/v0.19.0...v0.20.0) (2026-08-02)


### Bug Fixes

* **iptv:** filter resolved catalogs by media_kind in catalogs_for_kinds ([6bd41da](https://github.com/lostb1t/remux/commit/6bd41daac68fa27205dc656fffdb12ab7648c24a))
* **probe:** skip short-duration check for audio-only streams; skip remuxdb for non-movie/episode kinds ([546c268](https://github.com/lostb1t/remux/commit/546c26836faa393bfc303173ea9af6fdb4ec1816))
* **streams:** resolve addon streams on demand in stream/HLS path ([9fdbb03](https://github.com/lostb1t/remux/commit/9fdbb038e3d39669289a6407c42256bf2f433e56))


### Features

* add remux-desktop system tray app with cross-platform builds and runtime ffmpeg download ([#160](https://github.com/lostb1t/remux/issues/160)) ([8afeb6c](https://github.com/lostb1t/remux/commit/8afeb6c645796ad53431acbcc127957dbe901a5f))
* re-sort Next Up by effective key to surface newly released episodes ([#134](https://github.com/lostb1t/remux/issues/134)) ([85b3491](https://github.com/lostb1t/remux/commit/85b3491459b28b3b4c9114dbec6045bbdc437520))

# [0.19.0](https://github.com/lostb1t/remux/compare/v0.18.1...v0.19.0) (2026-07-31)


### Bug Fixes

* **addons:** surface clear errors when addon manifest is unreachable or 404 ([8328499](https://github.com/lostb1t/remux/commit/8328499c0c1e4393aad6d70e2f351fd191d33de7))
* **quickconnect:** respect userId param in authorize endpoint ([d8ea13b](https://github.com/lostb1t/remux/commit/d8ea13b1969a24fd0b7fb5ae0cb554dba31aeac7))
* **resume:** re-watched items missing from continue watching, closes [#150](https://github.com/lostb1t/remux/issues/150) ([c1260c4](https://github.com/lostb1t/remux/commit/c1260c45cedbcf1d20e6f559eb0a4262f454dbe8))
* **stream_groups:** resolve group UUIDs to items via per-user store mapping ([29340d6](https://github.com/lostb1t/remux/commit/29340d6bbcafc76846a856980bbacb07b78e4817))
* stremio custom type anime catalogs ([#137](https://github.com/lostb1t/remux/issues/137)) ([7b77b55](https://github.com/lostb1t/remux/commit/7b77b55df782b5fbd74c875a97870d2ddd1e6f0e))
* **tests:** add missing TranscodeSession fields in engine test fixture ([77cb477](https://github.com/lostb1t/remux/commit/77cb4773e52d748c46e929f2f142413c09144b6d))
* **transcode:** apply HDR colour treatment in CPU overlay subtitle filter_complex ([4014f6e](https://github.com/lostb1t/remux/commit/4014f6e65b968ccd515509c9e70b00c799ed70c5))
* **transcode:** use overlay_qsv for QSV subtitle burn-in instead of SW decode ([691a2cc](https://github.com/lostb1t/remux/commit/691a2cc2e82c200c38c6092a4d58f9b4f88cb5fe))


### Features

* **admin:** restrict dashboard access to admin users only, closes [#152](https://github.com/lostb1t/remux/issues/152) ([9eb06f3](https://github.com/lostb1t/remux/commit/9eb06f350b4fa73479123f1a4f2d4f84f18529ee))
* **sessions:** expose bitrate, framerate, and hw accel in TranscodingInfo, closes [#153](https://github.com/lostb1t/remux/issues/153) ([e2cbd44](https://github.com/lostb1t/remux/commit/e2cbd4414a3bfee8c62bf0da02acf3d2af9e618f))

## [0.18.1](https://github.com/lostb1t/remux/compare/v0.18.0...v0.18.1) (2026-07-27)


### Bug Fixes

* **container:** normalize ffprobe format_name and propagate probe container through cache-hit path ([ee4b3a6](https://github.com/lostb1t/remux/commit/ee4b3a6bbb9508042f314f341aa5da5345a19dee))
* propagate bitrate and size from probe_data in From<db::Media> ([5ec8977](https://github.com/lostb1t/remux/commit/5ec8977d75bd59944c5f63a728529882f40f5b44))

## [0.18.1](https://github.com/lostb1t/remux/compare/v0.18.0...v0.18.1) (2026-07-27)


### Bug Fixes

* **container:** normalize ffprobe format_name and propagate probe container through cache-hit path ([ee4b3a6](https://github.com/lostb1t/remux/commit/ee4b3a6bbb9508042f314f341aa5da5345a19dee))
* propagate bitrate and size from probe_data in From<db::Media> ([5ec8977](https://github.com/lostb1t/remux/commit/5ec8977d75bd59944c5f63a728529882f40f5b44))

# [0.18.0](https://github.com/lostb1t/remux/compare/v0.17.0...v0.18.0) (2026-07-27)


### Bug Fixes

* **addons:** add per-addon stream elapsed logging and cap remuxdb probe at 10s ([9e5ae3f](https://github.com/lostb1t/remux/commit/9e5ae3f9a4603f5a72ed2dfab593cea485fa6795))
* **auth:** deserialize AuthenticateUserByName fields case-insensitively ([25a78e3](https://github.com/lostb1t/remux/commit/25a78e3460a74cd87b237f38b1d77494cc102f47))


### Features

* **stream-groups:** add AudioLanguage rule from probe_data ([#145](https://github.com/lostb1t/remux/issues/145)) ([9a8ef11](https://github.com/lostb1t/remux/commit/9a8ef1140adcef7309fea8a50bd594daf4a7b692))
* **stream-groups:** add Size rule and propagate torznab size ([#144](https://github.com/lostb1t/remux/issues/144)) ([2534c95](https://github.com/lostb1t/remux/commit/2534c9590296a9a513ad26184abbe13ed6fefb1a))

# [0.17.0](https://github.com/lostb1t/remux/compare/v0.16.0...v0.17.0) (2026-07-25)


### Bug Fixes

* close IN() paren before GROUP BY in get_similar_by_genres ([c257871](https://github.com/lostb1t/remux/commit/c257871dbffcc18cf4f68751099f778b7f97577d))
* **items:** resolve parent_id aliases before child query ([#123](https://github.com/lostb1t/remux/issues/123)) ([4d7e1fc](https://github.com/lostb1t/remux/commit/4d7e1fc8c1e596de93e3df0e71f31d0a36b1e98e))
* Jellyfin API compat — Filters2 endpoint and list filter params ([#136](https://github.com/lostb1t/remux/issues/136)) ([177d13b](https://github.com/lostb1t/remux/commit/177d13b62f395896784ef144bc44ca35a676b16a))
* **loudness:** default normalize_audio_loudness to false ([#139](https://github.com/lostb1t/remux/issues/139)) ([31dbf81](https://github.com/lostb1t/remux/commit/31dbf8146c83719e4997cf8e9e5a13163d101e82))
* resolve parent_id aliases in items_flat; add persist_from_store test ([1048e72](https://github.com/lostb1t/remux/commit/1048e729d11e2e48ff6b6c99b167cf7cc859c3f7))
* resolve temp search IDs in shows seasons and episodes endpoints ([c48a346](https://github.com/lostb1t/remux/commit/c48a346fcfae8da1997d4076d982ebcf2cac19c4))
* use series_imdb for remuxdb episode probe lookup ([f1bf79c](https://github.com/lostb1t/remux/commit/f1bf79c0a1de6506cd9581bbc404f48a75ba59fd))


### Features

* **addons:** load and merge RemuxDB probe data during stream refresh ([#122](https://github.com/lostb1t/remux/issues/122)) ([81b3a5c](https://github.com/lostb1t/remux/commit/81b3a5ca3eacdfba9df9ef26c5638cb2fc383867))
* **engine:** add configurable loudness normalization ([#125](https://github.com/lostb1t/remux/issues/125)) ([0a2794e](https://github.com/lostb1t/remux/commit/0a2794e7a8dab434407d9a70456c0668861f643f))
* implement field locking to prevent metadata provider overwrites ([#135](https://github.com/lostb1t/remux/issues/135)) ([ed19f0a](https://github.com/lostb1t/remux/commit/ed19f0a892250cdb07fbced134ec7bc4b43393d7))
* implement SendMessageCommand via WebSocket GeneralCommand ([5a5028a](https://github.com/lostb1t/remux/commit/5a5028a8e05a3edbd95eb76817c6be5bac2d04f3))
* parse mediaInfo from stream behaviorHints into probe_data ([eeccb3a](https://github.com/lostb1t/remux/commit/eeccb3a76d752afb447a8ddbeca951d76467e417))
* serve /web/manifest.json for webos client compat (closes [#117](https://github.com/lostb1t/remux/issues/117)) ([3cc0f86](https://github.com/lostb1t/remux/commit/3cc0f86b88393b238056d5c2b90bdd8511b0d809))
* **users:** per-user subtitle mode and language preference ([#124](https://github.com/lostb1t/remux/issues/124)) ([85b40c0](https://github.com/lostb1t/remux/commit/85b40c0ec20c2721a09341899f963b7520722d1b))


### Performance Improvements

* popularity sort via CTE + UNION ALL coroutine (no global sort, all items included) ([#130](https://github.com/lostb1t/remux/issues/130)) ([7ec439c](https://github.com/lostb1t/remux/commit/7ec439c032c7765cb21d9589ab9503fbff9cc3d1))

# [0.16.0](https://github.com/lostb1t/remux/compare/v0.15.0...v0.16.0) (2026-07-21)


### Bug Fixes

* preserve recently aired episodes in release filter ([#121](https://github.com/lostb1t/remux/issues/121)) ([8aa96bb](https://github.com/lostb1t/remux/commit/8aa96bbbb853109ccdd17292113bd40950e657f6))
* **users:** add ON DELETE CASCADE to jellyfin_display_prefs ([b656851](https://github.com/lostb1t/remux/commit/b656851b1a85c652f715d590aa7c4dbef4cc3392))
* **users:** pre-delete display prefs and devices before user deletion ([1a55cbd](https://github.com/lostb1t/remux/commit/1a55cbd912ab659450a8abe90467841aaf5ee550))


### Features

* **addons:** per-user addon scoping with default list override (closes [#108](https://github.com/lostb1t/remux/issues/108)) ([#118](https://github.com/lostb1t/remux/issues/118)) ([93a4724](https://github.com/lostb1t/remux/commit/93a4724e010acd38329c22ed3c0ba1bdf5ad0a6c))
* **dashboard:** add playlists as a collection media kind (closes [#115](https://github.com/lostb1t/remux/issues/115)) ([4279f33](https://github.com/lostb1t/remux/commit/4279f3313d7fd785dc25e89e8c014005420bf63d))
* **dashboard:** simplify RemuxDB settings page, remove token field, add description ([18f9e52](https://github.com/lostb1t/remux/commit/18f9e523ef0a43e22a76c23b6a5a759adebf1c5b))
* **tasks:** add purge category with per-type purge tasks (closes [#113](https://github.com/lostb1t/remux/issues/113)) ([b6e903d](https://github.com/lostb1t/remux/commit/b6e903dcb3045a428a37732908211116fc92f1bf))

# [0.15.0](https://github.com/lostb1t/remux/compare/v0.14.0...v0.15.0) (2026-07-16)


### Bug Fixes

* **deezer:** deduplicate regional album variants by title (closes [#107](https://github.com/lostb1t/remux/issues/107)) ([56f9dcf](https://github.com/lostb1t/remux/commit/56f9dcf8b18686ef9ac6b02e4fb2482719e886e5))
* default audio language [#102](https://github.com/lostb1t/remux/issues/102) ([d598e6c](https://github.com/lostb1t/remux/commit/d598e6c2f979883ac1228de41d5e60ad5c6fb548))
* hide empty collections ([#109](https://github.com/lostb1t/remux/issues/109)) ([32e5c58](https://github.com/lostb1t/remux/commit/32e5c58f30817113c1091fca1575b8165ec2419d))
* **hls:** implement live.m3u8 handler and force AAC for live channels ([#112](https://github.com/lostb1t/remux/issues/112)) ([ca2efc8](https://github.com/lostb1t/remux/commit/ca2efc8553cb5656cadda6019bbb26b3d23efd4b))
* import stremio catalogs with unknown types (StarWars, Marvel, etc) ([c2250b0](https://github.com/lostb1t/remux/commit/c2250b0db6d6eb9b9332c6443c66d1dde75ed004))
* inverted play_default_audio_track logic clears language preference when it should not ([f50d19a](https://github.com/lostb1t/remux/commit/f50d19a939988d69db75d918040ee8df20aaa6c3)), closes [#102](https://github.com/lostb1t/remux/issues/102)
* **iptv:** use stream URL as channel UUID to preserve duplicate channels (closes [#78](https://github.com/lostb1t/remux/issues/78)) ([3557803](https://github.com/lostb1t/remux/commit/3557803c829e1eadd4f9ca85ffca7fd31d389c6a))
* **meta:** treat series with null status as active for episode refresh ([be1e46f](https://github.com/lostb1t/remux/commit/be1e46f20ba303298e120f79579f8129c816126f))
* **music:** improve music client compatibility ([#111](https://github.com/lostb1t/remux/issues/111)) ([8e55325](https://github.com/lostb1t/remux/commit/8e55325cff74eeec831d9957b7c16b152afdf9e4))
* remuxdb_url serde default so it applies without explicit config ([85cfdf4](https://github.com/lostb1t/remux/commit/85cfdf4efa97e9f8dd5856145372eaaf1e589e07))
* **stream_groups:** expose parent item ID on all MediaSources, not just the first ([3ed7594](https://github.com/lostb1t/remux/commit/3ed759409edf902a03c00bf74d9d6f1fc4d6bf16))
* **stream_groups:** restore per-source IDs in GetItems and redirect StreamGroup item lookups to parent ([4a98576](https://github.com/lostb1t/remux/commit/4a98576b31335e66d5e37299e90d37e38eb2ae09))
* **stream_groups:** use StreamGroup UUID as client-facing source ID ([ea40784](https://github.com/lostb1t/remux/commit/ea40784b0cdaa454078a80bde2f5c7054682fcbd))
* **stremio:** add type field to Trailer ([5c87693](https://github.com/lostb1t/remux/commit/5c8769302cf44e621c16c4a7ddbe50736cb5919c))
* **stremio:** make Trailer fields optional to handle missing source ([171d3fb](https://github.com/lostb1t/remux/commit/171d3fb5be0da35dd52b62eac9af5fb06b76ead9))
* **stremio:** preserve inner value when formatting MediaType::Unknown ([9a6b91e](https://github.com/lostb1t/remux/commit/9a6b91e287aa612262f1e7eb5215b860e933b302))


### Features

* add kitsu_id, is_anamorphic, level, ref_frames to remuxdb mediainfo payload ([9f82f6a](https://github.com/lostb1t/remux/commit/9f82f6af846ce5846e9acd3942dbe43d25db6ced))
* add per-user video transcoding toggle to user form ([#94](https://github.com/lostb1t/remux/issues/94)) ([fbe7cd8](https://github.com/lostb1t/remux/commit/fbe7cd8eaa91795eeaad751d2138b660996d88ce))
* add remuxdb sdk module ([c3495e2](https://github.com/lostb1t/remux/commit/c3495e2f5105d92fde413808d1874b8900becf04))
* dark theme ([#98](https://github.com/lostb1t/remux/issues/98)) ([92db73f](https://github.com/lostb1t/remux/commit/92db73f8347f288ca9d0a86685fdef006063848f))
* remote control ([#96](https://github.com/lostb1t/remux/issues/96)) ([36c8992](https://github.com/lostb1t/remux/commit/36c8992154ef15d78e3a5c7db900407eaba90f0f))
* submit probe data to remuxdb after live probes ([8a08139](https://github.com/lostb1t/remux/commit/8a08139d3f721ecdea228113da5fd717c0c3c8e4))
* wire preferred_metadata_language to TMDB addon and expose in dashboard (closes [#81](https://github.com/lostb1t/remux/issues/81)) ([bd143de](https://github.com/lostb1t/remux/commit/bd143dea35a1813272a0b1ba79ac7e2418ff447f))

# [0.14.0](https://github.com/lostb1t/remux/compare/v0.13.0...v0.14.0) (2026-07-10)


### Bug Fixes

* use effective stream for id/path/subtitles when probe falls back to a different stream ([411c173](https://github.com/lostb1t/remux/commit/411c1731f0ab26d60508667515d03f9357b8cd07))


### Features

* reject suspiciously short remote streams as probe failures ([#90](https://github.com/lostb1t/remux/issues/90)) ([7e37491](https://github.com/lostb1t/remux/commit/7e3749137a50955818af1b3b68328875e4881f1b))


### Performance Improvements

* diff media relations instead of delete+reinsert to reduce WAL pressure ([7078e31](https://github.com/lostb1t/remux/commit/7078e31ec7cabafd71dc084b9a46461bbafd95c2))

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
