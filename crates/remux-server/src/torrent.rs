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
        })
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

fn parse_season_episode(s: &str) -> Option<(Option<u32>, u32)> {
    static SE_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)[sS](\d{1,4})[eE](\d{1,4})").unwrap()
    });
    if let Some(caps) = SE_RE.captures(s) {
        let season = caps[1]
            .parse::<u32>()
            .ok();
        let episode = caps[2]
            .parse::<u32>()
            .ok()?;
        return Some((season, episode));
    }

    static X_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)(?:^|[\s._\-\[])(\d{1,4})x(\d{1,4})(?:[\s._\-\])]|$)")
            .unwrap()
    });
    if let Some(caps) = X_RE.captures(s) {
        let season = caps[1]
            .parse::<u32>()
            .ok();
        let episode = caps[2]
            .parse::<u32>()
            .ok()?;
        return Some((season, episode));
    }

    static EP_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)(?:^|[\s._\-\[])(?:ep|episode)[._\-\s]*(\d{1,4})(?:[\s._\-\])]|$)",
        )
        .unwrap()
    });
    if let Some(caps) = EP_RE.captures(s) {
        let episode = caps[1]
            .parse::<u32>()
            .ok()?;
        return Some((None, episode));
    }

    static E_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)(?:^|[\s._\-\[])[eE](\d{1,4})(?:[\s._\-\])]|$)")
            .unwrap()
    });
    if let Some(caps) = E_RE.captures(s) {
        let episode = caps[1]
            .parse::<u32>()
            .ok()?;
        return Some((None, episode));
    }

    None
}

fn matches_episode_pattern(filename: &str, wanted: &str) -> bool {
    let Some((wanted_season, wanted_ep)) = parse_season_episode(wanted) else {
        return false;
    };

    // 1. Try parsing season & episode directly from filename
    if let Some((file_season, file_ep)) = parse_season_episode(filename) {
        if file_ep == wanted_ep {
            if wanted_season.is_none()
                || file_season.is_none()
                || wanted_season == file_season
            {
                return true;
            }
        }
    }

    // 2. Check token/format matches in lowercase filename
    let lower = filename.to_ascii_lowercase();
    let mut patterns = Vec::new();
    if let Some(s) = wanted_season {
        patterns.push(format!("s{:02}e{:02}", s, wanted_ep));
        patterns.push(format!("s{}e{}", s, wanted_ep));
        patterns.push(format!("{}x{:02}", s, wanted_ep));
        patterns.push(format!("{}x{}", s, wanted_ep));
    }
    patterns.push(format!("ep{:02}", wanted_ep));
    patterns.push(format!("ep{}", wanted_ep));
    patterns.push(format!("e{:02}", wanted_ep));

    for p in &patterns {
        if lower.contains(p) {
            return true;
        }
    }

    // 3. Absolute numbering match (e.g. "0001", "001", "01" for episode 1)
    let ep_strs = [
        format!("{:04}", wanted_ep),
        format!("{:03}", wanted_ep),
        format!("{:02}", wanted_ep),
    ];
    for ep_str in ep_strs {
        if let Some(pos) = lower.find(&ep_str) {
            let before = if pos == 0 {
                None
            } else {
                lower[..pos]
                    .chars()
                    .last()
            };
            let after_pos = pos + ep_str.len();
            let after = if after_pos >= lower.len() {
                None
            } else {
                lower[after_pos..]
                    .chars()
                    .next()
            };
            let before_ok = before.map_or(true, |c| !c.is_ascii_alphanumeric());
            let after_ok =
                after.map_or(true, |c| !c.is_ascii_alphanumeric() || c == 'v');
            if before_ok && after_ok {
                if wanted_ep == 720
                    || wanted_ep == 1080
                    || wanted_ep == 480
                    || (wanted_ep >= 1990 && wanted_ep <= 2030)
                {
                    continue;
                }
                return true;
            }
        }
    }

    false
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

    if let Some(wanted) = wanted_file {
        let matching_videos: Vec<(usize, &TorrentFile)> = videos
            .iter()
            .copied()
            .filter(|(_, file)| matches_episode_pattern(&file.name, wanted))
            .collect();
        if matching_videos.len() == 1 {
            return Ok(matching_videos[0].0);
        }
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

    #[test]
    fn test_select_file_index_matches_season_pack_episode_pattern() {
        let files = vec![
            file("Season 1/Show.Name.S01E01.1080p.mkv", 1_200),
            file("Season 1/Show.Name.S01E02.1080p.mkv", 1_210),
            file("Season 1/Show.Name.S01E03.1080p.mkv", 1_190),
            file("Season 1/Show.Name.S01E04.1080p.mkv", 1_205),
        ];

        // Wanted by episode query or filename hint from another release
        assert_eq!(
            select_file_index(&files, None, Some("Show.Name.S01E03.720p.HDTV.mkv"))
                .unwrap(),
            2
        );
        assert_eq!(select_file_index(&files, None, Some("S01E03")).unwrap(), 2);
        assert_eq!(select_file_index(&files, None, Some("1x03")).unwrap(), 2);

        // Anime absolute numbering season pack
        let anime_files = vec![
            file("One Piece 0001.mkv", 500),
            file("One Piece 0002.mkv", 510),
            file("One Piece 0352.mkv", 520),
        ];
        assert_eq!(
            select_file_index(&anime_files, None, Some("One Piece S01E01")).unwrap(),
            0
        );
        assert_eq!(
            select_file_index(&anime_files, None, Some("S01E01")).unwrap(),
            0
        );
    }
}
