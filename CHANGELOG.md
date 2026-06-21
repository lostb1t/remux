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
