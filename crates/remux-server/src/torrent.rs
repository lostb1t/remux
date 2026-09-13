use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, Session, SessionOptions,
    SessionPersistenceConfig, TorrentStatsState,
    api::{Api, TorrentIdOrHash},
    dht::PersistentDhtConfig,
    http_api::HttpApi,
};
use tracing::{debug, warn};

#[derive(Clone, Debug)]
struct TorrentFile {
    name: String,
    length: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SidecarSubtitleFile {
    file_idx: usize,
    path: String,
    language: Option<String>,
    is_forced: bool,
    is_hearing_impaired: bool,
}

pub struct TorrentManager {
    session: Arc<Session>,
    http_port: u16,
    users: tokio::sync::Mutex<
        std::collections::HashMap<String, std::sync::Weak<TorrentLease>>,
    >,
}

/// Shared by playback sessions and response bodies using this exact torrent.
pub struct TorrentLease {
    manager: Arc<TorrentManager>,
    hash: String,
}

impl Drop for TorrentLease {
    fn drop(&mut self) {
        let manager = self
            .manager
            .clone();
        let hash = self
            .hash
            .clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                // Allow a seek/reconnect or the next episode to acquire the
                // same torrent before releasing its peers and downloaded data.
                tokio::time::sleep(Duration::from_secs(30)).await;
                if let Err(error) = manager
                    .delete_if_unused(&hash)
                    .await
                {
                    warn!(%hash, "failed to release torrent: {error:#}");
                }
            });
        }
    }
}

impl TorrentManager {
    pub async fn new(
        data_dir: PathBuf,
        cache_dir: PathBuf,
        http_port: Option<u16>,
        disable_dht: bool,
        peer_port: Option<u16>,
    ) -> Result<Self> {
        let session = Session::new_with_opts(
            data_dir,
            SessionOptions {
                disable_dht,
                disable_dht_persistence: disable_dht,
                listen_port_range: peer_port.map(|p| p..p + 10),
                persistence: Some(SessionPersistenceConfig::Json {
                    folder: Some(cache_dir.join("rqbit")),
                }),
                dht_config: Some(PersistentDhtConfig {
                    config_filename: Some(cache_dir.join("dht.json")),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await?;

        // None → let the OS pick a free ephemeral port.
        let bind_port = http_port.unwrap_or(0);
        let listener =
            tokio::net::TcpListener::bind(format!("127.0.0.1:{}", bind_port)).await?;

        let bound_port = listener
            .local_addr()?
            .port();

        let api = Api::new(session.clone(), None, None);
        let http_api = HttpApi::new(api, None);
        tokio::spawn(http_api.make_http_api_and_run(listener, None));

        debug!(port = bound_port, "torrent HTTP server listening");
        Ok(Self {
            session,
            http_port: bound_port,
            users: Default::default(),
        })
    }

    pub async fn acquire(self: &Arc<Self>, hash: &str) -> Arc<TorrentLease> {
        // Magnet hashes may be hex or base32; librqbit lists them as hex.
        let hash = librqbit::Magnet::parse(&format!("magnet:?xt=urn:btih:{hash}"))
            .ok()
            .and_then(|magnet| magnet.as_id20())
            .map(|id| id.as_string())
            .unwrap_or_else(|| hash.to_ascii_lowercase());
        let mut users = self
            .users
            .lock()
            .await;
        if let Some(lease) = users
            .get(&hash)
            .and_then(std::sync::Weak::upgrade)
        {
            return lease;
        }
        let lease = Arc::new(TorrentLease {
            manager: self.clone(),
            hash: hash.clone(),
        });
        users.insert(hash, Arc::downgrade(&lease));
        lease
    }

    async fn delete_if_unused(&self, hash: &str) -> Result<()> {
        // Acquisition and deletion share the lock, so a new reader cannot
        // acquire a torrent between the last-user check and deletion.
        let mut users = self
            .users
            .lock()
            .await;
        if users
            .get(hash)
            .is_none_or(|lease| lease.strong_count() != 0)
        {
            return Ok(());
        }
        let api = Api::new(
            self.session
                .clone(),
            None,
            None,
        );
        if let Some(id) = api
            .api_torrent_list()
            .torrents
            .into_iter()
            .find(|torrent| {
                torrent
                    .info_hash
                    .eq_ignore_ascii_case(hash)
            })
            .and_then(|torrent| torrent.id)
        {
            api.api_torrent_action_delete(TorrentIdOrHash::Id(id))
                .await?;
        }
        users.remove(hash);
        Ok(())
    }

    pub async fn from_config(config: &crate::Config) -> Result<Self> {
        let data_dir = config
            .torrent_data_dir
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Config::resolve() must be called before TorrentManager::from_config"))?;
        Self::new(
            std::path::PathBuf::from(data_dir),
            config
                .data_dir
                .join("cache"),
            config.torrent_http_port,
            config.disable_dht,
            config.torrent_peer_port,
        )
        .await
    }

    fn managed_torrent_files(&self, info_hash: &str) -> Option<Vec<TorrentFile>> {
        let api = Api::new(
            self.session
                .clone(),
            None,
            None,
        );
        let torrent_id = api
            .api_torrent_list()
            .torrents
            .into_iter()
            .find(|torrent| {
                torrent
                    .info_hash
                    .eq_ignore_ascii_case(info_hash)
            })?
            .id?;
        api.api_torrent_details(TorrentIdOrHash::Id(torrent_id))
            .ok()?
            .files
            .map(|files| {
                files
                    .into_iter()
                    .map(|file| TorrentFile {
                        name: file.name,
                        length: file.length,
                    })
                    .collect()
            })
    }

    /// Gracefully shut down the librqbit session, releasing all sockets
    /// (including the DHT UDP socket). Call this before dropping the manager
    /// to avoid "address already in use" errors on restart.
    pub async fn shutdown(&self) {
        self.session
            .stop()
            .await;
    }

    /// Resolve a magnet URI (possibly with `&tr=`, `&file_idx=`, `&file=` params
    /// we encode) to a local `http://127.0.0.1:<port>/torrents/<id>/stream/<file_idx>` URL
    pub async fn resolve_url(&self, magnet: &str) -> Result<String> {
        let file_idx_override = parse_file_idx_param(magnet);
        let wanted_file = parse_file_param(magnet);
        debug!(
            magnet,
            ?wanted_file,
            ?file_idx_override,
            "resolving torrent"
        );

        let response = self
            .session
            .add_torrent(AddTorrent::from_url(magnet), Some(stream_only_options()))
            .await
            .context("failed to add torrent")?;

        let (torrent_id, handle) = match response {
            AddTorrentResponse::Added(id, h) => (id, h),
            AddTorrentResponse::AlreadyManaged(id, h) => (id, h),
            AddTorrentResponse::ListOnly(_) => {
                anyhow::bail!("unexpected ListOnly response")
            }
        };

        tokio::time::timeout(Duration::from_secs(30), handle.wait_until_initialized())
            .await
            .context("timed out waiting for torrent metadata")?
            .context("torrent initialization failed")?;

        let files = handle.with_metadata(|metadata| {
            metadata
                .file_infos
                .iter()
                .map(|file| TorrentFile {
                    name: file
                        .relative_filename
                        .to_string_lossy()
                        .into_owned(),
                    length: file.len,
                })
                .collect::<Vec<_>>()
        })?;
        let file_idx =
            select_file_index(&files, file_idx_override, wanted_file.as_deref())?;

        // Existing persisted torrents may have been created with every file
        // selected. Clear that natural queue as well; active FileStreams keep
        // requesting their own pieces independently.
        let api = Api::new(
            self.session
                .clone(),
            None,
            None,
        );
        api.api_torrent_action_update_only_files(
            TorrentIdOrHash::Id(torrent_id),
            &std::collections::HashSet::new(),
        )
        .await
        .context("failed to clear torrent file selection")?;
        if !matches!(
            handle
                .stats()
                .state,
            TorrentStatsState::Live
        ) {
            if let Err(error) = api
                .api_torrent_action_start(TorrentIdOrHash::Id(torrent_id))
                .await
            {
                // Another request may have started the torrent between the
                // state check and this action. Only suppress that race.
                if matches!(
                    handle
                        .stats()
                        .state,
                    TorrentStatsState::Live
                ) {
                    debug!(torrent_id, "torrent was started concurrently");
                } else {
                    return Err(error).context("failed to start torrent");
                }
            }
        }

        debug!(
            torrent_id,
            file_idx,
            file = %files[file_idx].name,
            file_count = files.len(),
            "selected torrent stream file"
        );

        Ok(format!(
            "http://127.0.0.1:{}/torrents/{}/stream/{}",
            self.http_port, torrent_id, file_idx
        ))
    }

    /// Delete managed torrents and their files, skipping any whose ID is in `active`.
    pub async fn delete_unused_with_files(
        &self,
        active: &std::collections::HashSet<usize>,
    ) -> Result<usize> {
        let users = self
            .users
            .lock()
            .await;
        let api = Api::new(
            self.session
                .clone(),
            None,
            None,
        );
        let ids: Vec<_> = api
            .api_torrent_list()
            .torrents
            .into_iter()
            .filter(|torrent| {
                !users
                    .get(
                        &torrent
                            .info_hash
                            .to_ascii_lowercase(),
                    )
                    .is_some_and(|lease| lease.strong_count() != 0)
            })
            .filter_map(|t| t.id)
            .filter(|id| !active.contains(id))
            .collect();
        let count = ids.len();
        for id in ids {
            if let Err(e) = api
                .api_torrent_action_delete(TorrentIdOrHash::Id(id))
                .await
            {
                warn!(id, "failed to delete torrent: {e:#}");
            }
        }
        Ok(count)
    }

    /// Parse the torrent ID out of a librqbit stream URL.
    /// Format: `http://127.0.0.1:{port}/torrents/{id}/stream/{file_idx}`
    pub fn torrent_id_from_url(url: &str) -> Option<usize> {
        let after_host = url
            .split_once("//")?
            .1
            .split_once('/')?
            .1;
        let mut parts = after_host.splitn(3, '/');
        if parts.next()? != "torrents" {
            return None;
        }
        parts
            .next()?
            .parse()
            .ok()
    }

    /// Apply upload/download speed limits.  0 = no limit (for download) or
    /// effectively-disabled (for upload — 1 bps is used since the API requires
    /// `NonZeroU32`).
    pub fn update_limits(&self, upload_kbps: i64, download_kbps: i64) {
        use std::num::NonZeroU32;
        // upload: 0 means "don't seed" — clamp to 1 bps (librqbit requires NonZero)
        let upload = NonZeroU32::new(if upload_kbps <= 0 {
            1
        } else {
            (upload_kbps as u32).saturating_mul(1024)
        });
        // download: 0 means unlimited → None
        let download = if download_kbps <= 0 {
            None
        } else {
            NonZeroU32::new((download_kbps as u32).saturating_mul(1024))
        };
        self.session
            .ratelimits
            .set_upload_bps(upload);
        self.session
            .ratelimits
            .set_download_bps(download);
    }
}

impl crate::stream::StreamInfo {
    /// Return supported subtitle files associated with this stream when its
    /// torrent metadata has already been initialized. This never starts a
    /// download; subtitle bytes are requested only if a client selects a track.
    pub(crate) fn subtitle_sidecars(
        &self,
        torrent: &TorrentManager,
    ) -> Vec<crate::addons::SubtitleInfo> {
        let crate::stream::StreamDescriptor::Torrent {
            info_hash,
            file_hint,
            file_idx,
            trackers,
        } = &self.descriptor
        else {
            return Vec::new();
        };
        let Some(files) = torrent.managed_torrent_files(info_hash) else {
            return Vec::new();
        };
        let Ok(selected_idx) =
            select_file_index(&files, *file_idx, file_hint.as_deref())
        else {
            return Vec::new();
        };
        select_sidecar_subtitles(&files, selected_idx)
            .into_iter()
            .map(|sidecar| crate::addons::SubtitleInfo {
                id: format!("torrent:{info_hash}:{}", sidecar.file_idx),
                url: Some(crate::stream::StreamDescriptor::Torrent {
                    info_hash: info_hash.clone(),
                    file_hint: Some(sidecar.path),
                    file_idx: Some(sidecar.file_idx),
                    trackers: trackers.clone(),
                }),
                lang: sidecar.language,
                is_forced: sidecar.is_forced,
                is_hi: sidecar.is_hearing_impaired,
            })
            .collect()
    }
}

fn stream_only_options() -> AddTorrentOptions {
    AddTorrentOptions {
        // An empty selection leaves piece ownership to librqbit's HTTP
        // FileStream. Metadata lookup therefore cannot start downloading or
        // allocating every file in a bundle.
        only_files: Some(Vec::new()),
        ..Default::default()
    }
}

fn is_video_file(name: &str) -> bool {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    remux_sdks::remux::VideoContainer::parse_known(&ext).is_some()
}

fn is_supported_sidecar_subtitle(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("srt"))
}

fn subtitle_language_from_name(name: &str) -> Option<String> {
    let stem = std::path::Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    stem.split(|character: char| !character.is_ascii_alphabetic())
        .rev()
        .find_map(|token| {
            let lowercase = token.to_ascii_lowercase();
            if matches!(
                lowercase.as_str(),
                "cc" | "default"
                    | "forced"
                    | "foreign"
                    | "sdh"
                    | "signs"
                    | "hearing"
                    | "impaired"
                    | "hearingimpaired"
            ) || token == "HI"
            {
                return None;
            }

            isolang::Language::from_639_1(&lowercase)
                .or_else(|| isolang::Language::from_639_3(&lowercase))
                .or_else(|| {
                    remux_sdks::remux::common_audio_languages()
                        .iter()
                        .find(|(code, _)| code.eq_ignore_ascii_case(&lowercase))
                        .and_then(|(_, name)| isolang::Language::from_name(name))
                })
                .or_else(|| {
                    isolang::languages().find(|language| {
                        language
                            .to_name()
                            .eq_ignore_ascii_case(&lowercase)
                    })
                })
                .and_then(|language| language.to_639_1())
                .map(str::to_string)
        })
}

fn subtitle_stem_matches_video(selected_stem: &str, subtitle_stem: &str) -> bool {
    !selected_stem.is_empty()
        && subtitle_stem
            .strip_prefix(selected_stem)
            .is_some_and(|suffix| {
                suffix.is_empty()
                    || suffix
                        .chars()
                        .next()
                        .is_some_and(|character| !character.is_ascii_alphanumeric())
            })
}

fn select_sidecar_subtitles(
    files: &[TorrentFile],
    selected_idx: usize,
) -> Vec<SidecarSubtitleFile> {
    let Some(selected) = files.get(selected_idx) else {
        return Vec::new();
    };
    let selected_name = selected
        .name
        .replace('\\', "/");
    let selected_path = std::path::Path::new(&selected_name);
    let selected_parent = selected_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    let selected_stem = selected_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let videos_in_parent = files
        .iter()
        .filter(|file| {
            if !is_video_file(&file.name) {
                return false;
            }
            let normalized = file
                .name
                .replace('\\', "/");
            std::path::Path::new(&normalized)
                .parent()
                .unwrap_or_else(|| std::path::Path::new(""))
                == selected_parent
        })
        .count();

    files
        .iter()
        .enumerate()
        .filter_map(|(file_idx, file)| {
            if file.length == 0
                || file.length > 20 * 1024 * 1024
                || !is_supported_sidecar_subtitle(&file.name)
            {
                return None;
            }
            let normalized = file
                .name
                .replace('\\', "/");
            let subtitle_path = std::path::Path::new(&normalized);
            let subtitle_parent = subtitle_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new(""));
            let subtitle_stem = subtitle_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let basename_matches =
                subtitle_stem_matches_video(&selected_stem, &subtitle_stem);
            let same_directory = subtitle_parent == selected_parent;
            let in_subtitle_directory = subtitle_parent
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.eq_ignore_ascii_case("subs")
                        || name.eq_ignore_ascii_case("subtitles")
                })
                && subtitle_parent
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new(""))
                    == selected_parent;
            // Generic names such as `Subs/2_English.srt` are safe only when
            // the selected directory contains a single video. Filename-matched
            // subtitles remain safe for episode packs and movie collections.
            if !basename_matches
                && !(videos_in_parent == 1 && (same_directory || in_subtitle_directory))
            {
                return None;
            }
            let tokens: Vec<&str> = subtitle_stem
                .split(|character: char| !character.is_ascii_alphanumeric())
                .filter(|token| !token.is_empty())
                .collect();
            Some(SidecarSubtitleFile {
                file_idx,
                path: normalized.clone(),
                language: subtitle_language_from_name(&normalized),
                is_forced: tokens
                    .iter()
                    .any(|token| matches!(*token, "forced" | "foreign" | "signs")),
                is_hearing_impaired: tokens
                    .iter()
                    .any(|token| {
                        matches!(*token, "sdh" | "cc" | "hi" | "hearingimpaired")
                    }),
            })
        })
        .collect()
}

fn select_file_index(
    files: &[TorrentFile],
    requested_idx: Option<usize>,
    wanted_file: Option<&str>,
) -> Result<usize> {
    if files.is_empty() {
        anyhow::bail!("torrent contains no files");
    }

    if let Some(wanted) = wanted_file {
        let wanted_is_sidecar = is_supported_sidecar_subtitle(wanted);
        if let Some((index, _)) = files
            .iter()
            .enumerate()
            .find(|(_, file)| {
                (is_video_file(&file.name)
                    || (wanted_is_sidecar && is_supported_sidecar_subtitle(&file.name)))
                    && (file
                        .name
                        .eq_ignore_ascii_case(wanted)
                        || std::path::Path::new(&file.name)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.eq_ignore_ascii_case(wanted)))
            })
        {
            return Ok(index);
        }
    }

    if let Some(index) = requested_idx.filter(|index| {
        files
            .get(*index)
            .is_some_and(|file| is_video_file(&file.name))
    }) {
        return Ok(index);
    }

    let mut videos: Vec<(usize, &TorrentFile)> = files
        .iter()
        .enumerate()
        .filter(|(_, file)| is_video_file(&file.name))
        .collect();
    if videos.len() == 1 {
        return Ok(videos[0].0);
    }

    videos.sort_by_key(|(_, file)| std::cmp::Reverse(file.length));
    if let [largest, second, ..] = videos.as_slice() {
        // Samples and extras are common, but similarly sized videos indicate
        // a real bundle and require an exact provider hint.
        if largest
            .1
            .length
            >= second
                .1
                .length
                .saturating_mul(2)
        {
            return Ok(largest.0);
        }
    }

    match requested_idx {
        Some(index) => anyhow::bail!(
            "torrent file index {index} does not identify a video and no unique video could be selected"
        ),
        None => anyhow::bail!(
            "torrent contains {} video files; a valid file index or filename is required",
            videos.len()
        ),
    }
}

/// Extract the `file=` query parameter we encode into our magnet URIs.
fn parse_file_param(magnet: &str) -> Option<String> {
    let query = magnet
        .split_once('?')?
        .1;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "file")
        .map(|(_, v)| v.into_owned())
}

/// Extract the `file_idx=` query parameter we encode into our magnet URIs.
fn parse_file_idx_param(magnet: &str) -> Option<usize> {
    let query = magnet
        .split_once('?')?
        .1;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "file_idx")
        .and_then(|(_, v)| {
            v.parse()
                .ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cleanup_waits_for_sessions_and_readers_and_respects_reacquisition() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            TorrentManager::new(
                dir.path()
                    .join("data"),
                dir.path()
                    .join("cache"),
                None,
                true,
                None,
            )
            .await
            .unwrap(),
        );
        let (server, guard, token) =
            crate::integration_test::authenticated_server().await;
        let sessions = &guard
            .0
            .sessions;
        // A tiny paused torrent exercises real librqbit deletion without
        // requiring DHT, trackers, peers, or a downloaded media fixture.
        manager.session.add_torrent(
            AddTorrent::from_bytes(&b"d4:infod6:lengthi1e4:name1:x12:piece lengthi16384e6:pieces20:....................ee"[..]),
            Some(AddTorrentOptions { paused: true, disable_trackers: true, ..Default::default() }),
        ).await.unwrap();
        let api = Api::new(
            manager
                .session
                .clone(),
            None,
            None,
        );
        let hash = api
            .api_torrent_list()
            .torrents[0]
            .info_hash
            .clone();
        let hash = hash.as_str();
        let other = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";

        // Requests may precede playback reports; these references must still
        // be released by stop. Two viewers share the very same torrent.
        sessions
            .retain_torrent("viewer-a", &manager, hash)
            .await;
        sessions
            .retain_torrent("viewer-b", &manager, hash)
            .await;
        let reader = manager
            .acquire(hash)
            .await;
        let weak = Arc::downgrade(&reader);
        sessions
            .stop("viewer-a")
            .await;
        manager
            .delete_if_unused(hash)
            .await
            .unwrap();
        assert!(
            weak.upgrade()
                .is_some()
        );
        assert_eq!(
            api.api_torrent_list()
                .torrents
                .len(),
            1
        );
        assert_eq!(
            manager
                .delete_unused_with_files(&Default::default())
                .await
                .unwrap(),
            0
        );
        drop(reader);
        assert!(
            weak.upgrade()
                .is_some(),
            "viewer-b still owns the torrent"
        );

        // Changing a selected source doesn't change the recorded reference
        // for an existing session; both actual sources are retained.
        sessions
            .retain_torrent("viewer-b", &manager, other)
            .await;
        sessions
            .stop("viewer-b")
            .await;
        assert!(
            weak.upgrade()
                .is_none()
        );
        let next_episode = manager
            .acquire(hash)
            .await;
        manager
            .delete_if_unused(hash)
            .await
            .unwrap();
        assert!(
            manager
                .users
                .lock()
                .await
                .contains_key(hash)
        );
        assert_eq!(
            api.api_torrent_list()
                .torrents
                .len(),
            1
        );
        drop(next_episode);
        manager
            .delete_if_unused(hash)
            .await
            .unwrap();
        manager
            .delete_if_unused(other)
            .await
            .unwrap();
        assert!(
            manager
                .users
                .lock()
                .await
                .is_empty()
        );
        assert!(
            api.api_torrent_list()
                .torrents
                .is_empty()
        );

        // Exercise real playback reports too: a start replaces the previous
        // session on this device, and a stop releases the replacement.
        let media = crate::integration_test::insert_test_source(&guard.0).await;
        let auth = crate::integration_test::auth_header_with_token(&token);
        for id in ["old-session", "new-session"] {
            sessions
                .retain_torrent(id, &manager, hash)
                .await;
            server
                .post("/sessions/playing")
                .add_header(
                    http::header::AUTHORIZATION,
                    http::HeaderValue::from_str(&auth).unwrap(),
                )
                .json(&serde_json::json!({ "ItemId": media.id, "PlaySessionId": id }))
                .await
                .assert_status(http::StatusCode::NO_CONTENT);
        }
        assert!(
            sessions
                .get("old-session")
                .is_none()
        );
        let lease = manager
            .acquire(hash)
            .await;
        let weak = Arc::downgrade(&lease);
        drop(lease);
        server.post("/sessions/playing/stopped")
            .add_header(http::header::AUTHORIZATION, http::HeaderValue::from_str(&auth).unwrap())
            .json(&serde_json::json!({ "ItemId": media.id, "PlaySessionId": "new-session" }))
            .await.assert_status(http::StatusCode::NO_CONTENT);
        assert!(
            weak.upgrade()
                .is_none(),
            "both removed sessions must release their references"
        );
        sessions
            .retain_torrent("orphan-request", &manager, hash)
            .await;
        let lease = manager
            .acquire(hash)
            .await;
        let weak = Arc::downgrade(&lease);
        drop(lease);
        let cleanup = sessions
            .clone()
            .spawn_cleanup_task(Duration::from_millis(1), Duration::ZERO);
        tokio::time::timeout(Duration::from_secs(2), async {
            while weak
                .upgrade()
                .is_some()
            {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("unclaimed requests must expire");
        cleanup.abort();
        manager
            .shutdown()
            .await;
    }

    fn file(name: &str, length: u64) -> TorrentFile {
        TorrentFile {
            name: name.to_string(),
            length,
        }
    }

    #[test]
    fn bundle_uses_exact_requested_file() {
        let files = vec![
            file("Bundle/Movie.One.mkv", 2_000),
            file("Bundle/Movie.Two.mkv", 2_100),
            file("Bundle/Movie.Three.mkv", 1_900),
        ];

        assert_eq!(
            select_file_index(&files, Some(0), Some("Movie.Two.mkv")).unwrap(),
            1
        );
        assert_eq!(select_file_index(&files, Some(2), None).unwrap(), 2);
    }

    #[test]
    fn bundle_rejects_ambiguous_or_non_video_indexes() {
        let files = vec![
            file("Bundle/Movie.One.mkv", 2_000),
            file("Bundle/release.nfo", 1),
            file("Bundle/Movie.Two.mkv", 2_100),
        ];

        assert!(select_file_index(&files, Some(1), None).is_err());
        assert!(select_file_index(&files, Some(99), None).is_err());
        assert!(select_file_index(&files, None, None).is_err());
    }

    #[test]
    fn single_feature_release_ignores_samples() {
        let files = vec![
            file("Release/sample.mkv", 100),
            file("Release/Movie.mkv", 2_000),
            file("Release/subtitles.srt", 2),
        ];

        assert_eq!(select_file_index(&files, None, None).unwrap(), 1);
    }

    #[test]
    fn metadata_lookup_selects_no_files_for_download() {
        assert_eq!(stream_only_options().only_files, Some(Vec::new()));
    }

    #[test]
    fn exact_sidecar_hint_selects_the_subtitle_file() {
        let files = vec![
            file("Movie.mkv", 2_000),
            file("Subs/English.srt", 20),
            file("release.nfo", 1),
        ];

        assert_eq!(
            select_file_index(&files, Some(1), Some("Subs/English.srt")).unwrap(),
            1
        );
    }

    #[test]
    fn single_movie_release_exposes_supported_subtitle_directory() {
        let files = vec![
            file("Movie.mkv", 2_000),
            file("Subs/2_English.srt", 20),
            file("Subs/3_English.srt", 25),
            file("Subs/4_English.ass", 30),
        ];

        let subtitles = select_sidecar_subtitles(&files, 0);
        assert_eq!(subtitles.len(), 2);
        assert_eq!(subtitles[0].file_idx, 1);
        assert_eq!(
            subtitles[0]
                .language
                .as_deref(),
            Some("en")
        );
        assert_eq!(subtitles[1].file_idx, 2);
    }

    #[test]
    fn movie_bundle_rejects_ambiguous_generic_subtitles() {
        let files = vec![
            file("Movie.One.mkv", 2_000),
            file("Movie.Two.mkv", 2_100),
            file("Subs/2_English.srt", 20),
        ];

        assert!(select_sidecar_subtitles(&files, 0).is_empty());
        assert!(select_sidecar_subtitles(&files, 1).is_empty());
    }

    #[test]
    fn episode_pack_uses_only_filename_matched_subtitles() {
        let files = vec![
            file("Show.S01E01.mkv", 1_000),
            file("Show.S01E01.en.HI.forced.srt", 10),
            file("Show.S01E02.mkv", 1_000),
            file("Show.S01E02.en.srt", 10),
            file("Show.S01E010.en.srt", 10),
        ];

        let subtitles = select_sidecar_subtitles(&files, 0);
        assert_eq!(subtitles.len(), 1);
        assert_eq!(subtitles[0].file_idx, 1);
        assert_eq!(
            subtitles[0]
                .language
                .as_deref(),
            Some("en")
        );
        assert!(subtitles[0].is_forced);
        assert!(subtitles[0].is_hearing_impaired);
    }
}
