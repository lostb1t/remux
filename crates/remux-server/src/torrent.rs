use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use librqbit::api::{Api, TorrentIdOrHash};
use librqbit::http_api::HttpApi;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, Session, SessionOptions,
};
use tracing::debug;

pub struct TorrentManager {
    session: Arc<Session>,
    http_port: u16,
}

impl TorrentManager {
    pub async fn new(
        data_dir: PathBuf,
        http_port: Option<u16>,
        disable_dht: bool,
    ) -> Result<Self> {
        let session = Session::new_with_opts(
            data_dir,
            SessionOptions {
                disable_dht,
                disable_dht_persistence: disable_dht,
                ..Default::default()
            },
        )
        .await?;

        // None → let the OS pick a free ephemeral port.
        let bind_port = http_port.unwrap_or(0);
        let listener =
            tokio::net::TcpListener::bind(format!("127.0.0.1:{}", bind_port)).await?;

        let bound_port = listener.local_addr()?.port();

        let api = Api::new(session.clone(), None, None);
        let http_api = HttpApi::new(api, None);
        tokio::spawn(http_api.make_http_api_and_run(listener, None));

        debug!(port = bound_port, "torrent HTTP server listening");
        Ok(Self {
            session,
            http_port: bound_port,
        })
    }

    /// Gracefully shut down the librqbit session, releasing all sockets
    /// (including the DHT UDP socket). Call this before dropping the manager
    /// to avoid "address already in use" errors on restart.
    pub async fn shutdown(&self) {
        self.session.stop().await;
    }

    /// Resolve a magnet URI (possibly with a `&file=<name>` param we encode) to a
    /// local `http://127.0.0.1:<port>/torrents/<id>/stream/<file_idx>` URL
    pub async fn resolve_url(&self, magnet: &str) -> Result<String> {
        let wanted_file = parse_file_param(magnet);
        debug!(magnet, ?wanted_file, "resolving torrent");

        let opts = wanted_file.as_deref().map(|name| AddTorrentOptions {
            // Ask librqbit to download only the matching file so we don't pull the
            // whole torrent.  The regex is anchored at the end so "Movie.mkv" doesn't
            // match "Movie.mkv.nfo".
            only_files_regex: Some(format!("(?i){}$", regex::escape(name))),
            ..Default::default()
        });

        let response = self
            .session
            .add_torrent(AddTorrent::from_url(magnet), opts)
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

        // Find the file index to stream.
        let file_idx = handle
            .with_metadata(|meta| {
                if let Some(name) = wanted_file.as_deref() {
                    meta.file_infos
                        .iter()
                        .enumerate()
                        .find(|(_, fi)| {
                            fi.relative_filename
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map(|n| n.eq_ignore_ascii_case(name))
                                .unwrap_or(false)
                        })
                        .map(|(idx, _)| idx)
                        .unwrap_or(0)
                } else {
                    0
                }
            })
            .unwrap_or(0);

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
        let api = Api::new(self.session.clone(), None, None);
        let ids: Vec<_> = api
            .api_torrent_list()
            .torrents
            .into_iter()
            .filter_map(|t| t.id)
            .filter(|id| !active.contains(id))
            .collect();
        let count = ids.len();
        for id in ids {
            if let Err(e) = api.api_torrent_action_delete(TorrentIdOrHash::Id(id)).await
            {
                tracing::warn!(id, "failed to delete torrent: {e:#}");
            }
        }
        Ok(count)
    }

    /// Parse the torrent ID out of a librqbit stream URL.
    /// Format: `http://127.0.0.1:{port}/torrents/{id}/stream/{file_idx}`
    pub fn torrent_id_from_url(url: &str) -> Option<usize> {
        let after_host = url.split_once("//")?.1.split_once('/')?.1;
        let mut parts = after_host.splitn(3, '/');
        if parts.next()? != "torrents" {
            return None;
        }
        parts.next()?.parse().ok()
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
        self.session.ratelimits.set_upload_bps(upload);
        self.session.ratelimits.set_download_bps(download);
    }
}

/// Extract the `file=` query parameter we encode into our magnet URIs.
fn parse_file_param(magnet: &str) -> Option<String> {
    let query = magnet.split_once('?')?.1;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "file")
        .map(|(_, v)| v.into_owned())
}
