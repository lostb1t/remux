use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use super::{
    AddonKind, AddonMetadata, AddonPreset, AddonPresetRegistration, LyricSearchRequest,
    MediaKind, ResourceType,
};
use crate::db;
use remux_sdks::remux::{LyricDto, LyricLine, LyricMetadata, RemoteLyricInfoDto};

pub struct LrcLibPreset;

impl AddonPreset for LrcLibPreset {
    fn id(&self) -> &'static str {
        "lrclib"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "lrclib".to_string(),
            display_name: "LRCLIB".to_string(),
            description: "Free lyrics provider — synced and plain lyrics for tracks."
                .to_string(),
            icon: None,
            supported_resources: vec![ResourceType::Lyrics],
            supported_types: vec![MediaKind::Track],
            options: vec![],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        _cfg: &serde_json::Value,
    ) -> Result<Arc<dyn AddonKind>> {
        let client = reqwest::Client::builder()
            .user_agent("remux-server/1.0")
            .build()
            .expect("failed to build HTTP client");
        Ok(Arc::new(LrcLibAddon { client }))
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(LrcLibPreset))
}

pub struct LrcLibAddon {
    client: reqwest::Client,
}

const BASE: &str = "https://lrclib.net/api";
const TICKS_PER_SECOND: f64 = 10_000_000.0;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibTrack {
    id: u64,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    duration: Option<f64>,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

fn parse_lrc(lrc: &str) -> Vec<LyricLine> {
    lrc.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix('[')?;
            let close = rest.find(']')?;
            let timestamp = &rest[..close];
            let text = rest[close + 1..].trim().to_string();
            let (mins_str, secs_str) = timestamp.split_once(':')?;
            let mins: f64 = mins_str.parse().ok()?;
            let secs: f64 = secs_str.parse().ok()?;
            let ticks = ((mins * 60.0 + secs) * TICKS_PER_SECOND) as i64;
            Some(LyricLine {
                text,
                start: Some(ticks),
            })
        })
        .collect()
}

fn plain_to_lines(plain: &str) -> Vec<LyricLine> {
    plain
        .lines()
        .map(|l| LyricLine {
            text: l.to_string(),
            start: None,
        })
        .collect()
}

fn track_to_dto(data: &LrcLibTrack) -> Option<LyricDto> {
    let is_synced = data.synced_lyrics.is_some();
    let lyrics = if let Some(lrc) = &data.synced_lyrics {
        parse_lrc(lrc)
    } else if let Some(plain) = &data.plain_lyrics {
        plain_to_lines(plain)
    } else {
        return None;
    };
    Some(LyricDto {
        metadata: LyricMetadata {
            title: data.track_name.clone(),
            artist: data.artist_name.clone(),
            album: data.album_name.clone(),
            length: data.duration.map(|d| (d * TICKS_PER_SECOND) as i64),
            is_synced: Some(is_synced),
        },
        lyrics,
    })
}

impl LrcLibAddon {
    async fn fetch_exact(&self, req: &LyricSearchRequest) -> Result<Option<LyricDto>> {
        let mut url = reqwest::Url::parse(&format!("{}/get", BASE))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("track_name", &req.title);
            if let Some(a) = &req.artist {
                q.append_pair("artist_name", a);
            }
            if let Some(a) = &req.album {
                q.append_pair("album_name", a);
            }
            if let Some(d) = req.duration_secs {
                q.append_pair("duration", &format!("{d:.2}"));
            }
        }
        tracing::debug!(url = %url, "lrclib: exact-match request");
        let resp = self.client.get(url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "lrclib /get returned error");
            return Ok(None);
        }
        Ok(resp
            .json::<LrcLibTrack>()
            .await
            .ok()
            .and_then(|t| track_to_dto(&t)))
    }
}

#[async_trait]
impl AddonKind for LrcLibAddon {
    fn id(&self) -> &'static str {
        "lrclib"
    }

    fn lyric_provider_id(&self) -> Option<String> {
        Some("lrclib".to_string())
    }

    async fn lyric_fetch(&self, req: &LyricSearchRequest) -> Result<Option<LyricDto>> {
        tracing::debug!(
            title = %req.title,
            artist = ?req.artist,
            album = ?req.album,
            duration = ?req.duration_secs,
            "lrclib: fetching lyrics"
        );

        if let Some(dto) = self.fetch_exact(req).await? {
            tracing::debug!(title = %req.title, "lrclib: exact match found");
            return Ok(Some(dto));
        }

        tracing::debug!(title = %req.title, "lrclib: exact match missed, trying search fallback");
        let results = self.lyric_search(req).await?;
        let first = results.into_iter().next().map(|r| r.lyrics);
        if first.is_some() {
            tracing::info!(title = %req.title, "lrclib: found via search fallback");
        } else {
            tracing::debug!(title = %req.title, "lrclib: no lyrics found");
        }
        Ok(first)
    }

    async fn lyric_search(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Vec<RemoteLyricInfoDto>> {
        let mut url = reqwest::Url::parse(&format!("{}/search", BASE))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("track_name", &req.title);
            if let Some(a) = &req.artist {
                q.append_pair("artist_name", a);
            }
            if let Some(a) = &req.album {
                q.append_pair("album_name", a);
            }
        }
        tracing::debug!(url = %url, "lrclib: search request");
        let resp = self.client.get(url).send().await?;
        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "lrclib /search returned error");
            return Ok(vec![]);
        }
        let tracks: Vec<LrcLibTrack> = resp.json().await?;
        tracing::debug!(
            count = tracks.len(),
            "lrclib: search returned {} results",
            tracks.len()
        );
        Ok(tracks
            .iter()
            .filter_map(|t| {
                track_to_dto(t).map(|dto| RemoteLyricInfoDto {
                    id: format!("lrclib_{}", t.id),
                    provider_name: "lrclib".into(),
                    lyrics: dto,
                })
            })
            .collect())
    }

    async fn lyric_get_by_id(&self, id: &str) -> Result<Option<LyricDto>> {
        let url = format!("{}/get/{}", BASE, id);
        tracing::debug!(id, "lrclib: get by id");
        let resp = self.client.get(&url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "lrclib /get/{id} returned error");
            return Ok(None);
        }
        Ok(resp
            .json::<LrcLibTrack>()
            .await
            .ok()
            .and_then(|t| track_to_dto(&t)))
    }
}
