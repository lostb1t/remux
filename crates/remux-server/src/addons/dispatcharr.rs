use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use regex::Regex;
use remux_sdks::stremio::MediaType as StremioMediaType;
use serde::Deserialize;
use std::{
    pin::Pin,
    sync::{Arc, LazyLock},
};
use tracing::debug;
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonOption, AddonOptionType,
    AddonPreset, AddonPresetRegistration, CatalogAddon, CatalogInfo, MediaKind,
    ResourceType, StreamAddon,
};
use crate::{AppContext, db};

// ---------------------------------------------------------------------------
// DispatcharrPreset
// ---------------------------------------------------------------------------

pub struct DispatcharrPreset;

impl AddonPreset for DispatcharrPreset {
    fn id(&self) -> &'static str {
        "dispatcharr"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "dispatcharr".to_string(),
            display_name: "Dispatcharr".to_string(),
            description: "Import live TV channels from a Dispatcharr instance, with every \
                assigned stream shown as a selectable version."
                .to_string(),
            icon: None,
            supported_resources: vec![
                AddonMetadata::simple_resource(ResourceType::Stream),
                AddonMetadata::simple_resource(ResourceType::Catalog),
            ],
            supported_types: vec![MediaKind::TvChannel],
            supported_resources_user: vec![],
            supported_types_user: vec![],
            options: vec![
                AddonOption {
                    id: "base_url".to_string(),
                    name: "Base URL".to_string(),
                    description: Some(
                        "Base URL of the Dispatcharr instance (e.g. http://dispatcharr:9191)."
                            .to_string(),
                    ),
                    required: true,
                    default: None,
                    kind: AddonOptionType::Url,
                },
                AddonOption {
                    id: "api_key".to_string(),
                    name: "API Key".to_string(),
                    description: Some(
                        "A Dispatcharr API key (Dispatcharr admin UI → API keys), sent as an \
                         X-API-Key header. Dedicated key recommended over reusing another \
                         integration's."
                            .to_string(),
                    ),
                    required: true,
                    default: None,
                    kind: AddonOptionType::Password,
                },
            ],
        }
    }

    fn from_cfg(
        &self,
        addon_id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let base_url = cfg["base_url"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("dispatcharr: base_url is required"))?
            .trim_end_matches('/')
            .to_string();
        let api_key = cfg["api_key"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("dispatcharr: api_key is required"))?
            .to_string();

        let addon = Arc::new(DispatcharrAddon {
            addon_id,
            base_url,
            api_key,
        });
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            catalog: Some(addon.clone()),
            stream: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(DispatcharrPreset))
}

// ---------------------------------------------------------------------------
// Dispatcharr API client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DispatcharrChannel {
    pub id: i64,
    pub uuid: String,
    pub name: String,
    pub channel_number: Option<f64>,
    /// FK to `EPGData` — the authoritative link to this channel's assigned
    /// guide data. The channel's own `tvg_id` is a copy that can go stale, so
    /// it isn't read; always resolve EPG through this instead.
    #[serde(default)]
    pub epg_data_id: Option<i64>,
    #[serde(default)]
    pub logo_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DispatcharrStream {
    pub id: i64,
    pub name: String,
    pub stream_hash: Option<String>,
    #[serde(default)]
    pub stream_stats: Option<serde_json::Value>,
}

/// Shared by every Dispatcharr API call, so they reuse one connection pool
/// and share a timeout instead of each opening its own untimed client.
pub(crate) static CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent("remux-server/1.0")
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build Dispatcharr client")
    });

/// Fetch every Dispatcharr channel.
///
/// With no query params this returns a plain array of every channel; DRF's
/// `{count,results}` envelope appears only when `page_size` is passed.
pub(crate) async fn fetch_channels(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<Vec<DispatcharrChannel>> {
    let resp = client
        .get(format!("{base_url}/api/channels/channels/"))
        .header("X-API-Key", token)
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<DispatcharrChannel>>()
        .await?;
    Ok(resp)
}

/// Fetch the full `Stream` objects assigned to one channel, in Dispatcharr's own
/// priority order (the order Dispatcharr itself returns them in — array order
/// on `Channel.streams` is the only priority signal Dispatcharr has).
pub(crate) async fn fetch_channel_streams(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    channel_id: i64,
) -> Result<Vec<DispatcharrStream>> {
    let resp = client
        .get(format!(
            "{base_url}/api/channels/channels/{channel_id}/streams/"
        ))
        .header("X-API-Key", token)
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<DispatcharrStream>>()
        .await?;
    Ok(resp)
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DispatcharrEpgData {
    pub id: i64,
    pub tvg_id: Option<String>,
}

/// Fetch every `EPGData` row — the per-source guide-channel bindings a
/// `Channel.epg_data_id` points at. Unlike `channels`/`streams`, this
/// endpoint returns a plain JSON array, not a paginated `{count,results}`
/// envelope.
pub(crate) async fn fetch_epg_data(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<Vec<DispatcharrEpgData>> {
    let resp = client
        .get(format!("{base_url}/api/epg/epgdata/"))
        .header("X-API-Key", token)
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<DispatcharrEpgData>>()
        .await?;
    Ok(resp)
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DispatcharrProgram {
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub title: String,
    #[serde(default)]
    pub sub_title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub tvg_id: String,
}

#[derive(Debug, Deserialize)]
struct EpgGridResponse {
    data: Vec<DispatcharrProgram>,
}

/// Fetch the EPG guide window (previous hour, current and next ~24h) for
/// every channel.
///
/// `ProgramData.tvg_id` joins to `EPGData.tvg_id`, so resolve a channel's
/// `epg_data_id` through `fetch_epg_data()` for the key — not the channel's
/// own `tvg_id` field, which is a copy and can go stale.
pub(crate) async fn fetch_epg_grid(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<Vec<DispatcharrProgram>> {
    let resp = client
        .get(format!("{base_url}/api/epg/grid/"))
        .header("X-API-Key", token)
        .send()
        .await?
        .error_for_status()?
        .json::<EpgGridResponse>()
        .await?;
    Ok(resp.data)
}

/// Frame height from a stream's `stream_stats` probe blob: a nested
/// `resolution.height` if present, otherwise the top-level `height`.
fn stat_height(stats: &serde_json::Value) -> Option<i64> {
    stats
        .get("resolution")
        .and_then(|r| r.get("height"))
        .and_then(|h| h.as_i64())
        .or_else(|| {
            stats
                .get("height")
                .and_then(|h| h.as_i64())
        })
}

/// `SD`/`HD`/`FHD`/`4K` bucket for a probed frame height.
fn resolution_label(stats: &serde_json::Value) -> Option<&'static str> {
    Some(match stat_height(stats)? {
        h if h >= 2000 => "4K",
        h if h >= 1000 => "FHD",
        h if h >= 700 => "HD",
        _ => "SD",
    })
}

/// Frame rate rounded to a whole number, e.g. `23.976` -> `24fps`.
fn fps_label(stats: &serde_json::Value) -> Option<String> {
    let fps = stats
        .get("source_fps")
        .and_then(|f| f.as_f64())
        .filter(|f| *f > 0.0)?;
    Some(format!("{}fps", fps.round() as i64))
}

/// A leading provider/country prefix, as in `ES: ` or `|ES| `.
static NAME_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:\|[A-Z]{2,3}\||[A-Z]{2,3}\s*[:|])\s*").unwrap()
});

/// Words in a stream name that describe quality rather than the channel:
/// `HD`, `FHD`, `4K`, `1080p`, `1080p50`, `50fps`, `RAW`.
static QUALITY_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:sd|hd|fhd|uhd|qhd|4k|8k|raw|\d{3,4}[pi]\d{0,3}|\d{2,3}fps)$")
        .unwrap()
});

/// Providers write quality markers in superscript (`ᴴᴰ`, `ᵁᴴᴰ`, `ᴿᴬᵂ`, `⁶⁰ᶠᵖˢ`).
/// Maps those characters to lowercase ASCII; anything else is unchanged.
fn fold_superscript(c: char) -> char {
    const SUPERSCRIPT: &str = concat!(
        "ᴬᴮᴰᴱᴳᴴᴵᴶᴷᴸᴹᴺᴼᴾᴿᵀᵁᵂ",
        "ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ",
        "⁰¹²³⁴⁵⁶⁷⁸⁹",
    );
    const ASCII: &str = concat!(
        "abdeghijklmnoprtuw",
        "abcdefghijklmnoprstuvwxyz",
        "0123456789",
    );
    SUPERSCRIPT
        .chars()
        .position(|s| s == c)
        .and_then(|i| {
            ASCII
                .chars()
                .nth(i)
        })
        .unwrap_or(c)
}

fn is_quality_word(word: &str) -> bool {
    let folded: String = word
        .chars()
        .map(fold_superscript)
        .flat_map(char::to_lowercase)
        .collect();
    QUALITY_WORD.is_match(folded.trim_matches(|c: char| !c.is_alphanumeric()))
}

/// The stream's name without its provider prefix or quality words, e.g.
/// `ES: LA 2 ᵁᴴᴰ` -> `LA 2`. Falls back to the raw name if nothing is left.
fn clean_stream_name(name: &str) -> String {
    let unprefixed = NAME_PREFIX.replace(name, "");
    let cleaned = unprefixed
        .split_whitespace()
        .filter(|word| !is_quality_word(word))
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned =
        cleaned.trim_matches(|c: char| c.is_whitespace() || "-–|:".contains(c));
    if cleaned.is_empty() {
        name.trim()
            .to_string()
    } else {
        cleaned.to_string()
    }
}

/// Version label shown in the picker: `FHD · 50fps · LA 2`. Resolution and
/// frame rate come from the probe stats and are left out when unprobed.
fn stream_title(stream: &DispatcharrStream) -> String {
    let stats = stream
        .stream_stats
        .as_ref();
    [
        stats
            .and_then(resolution_label)
            .map(str::to_owned),
        stats.and_then(fps_label),
        Some(clean_stream_name(&stream.name)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

pub(crate) fn channel_to_media(
    ch: &DispatcharrChannel,
    addon_id: Uuid,
    source_id: &str,
) -> db::Media {
    db::Media {
        id: Uuid::new_v5(&addon_id, format!("channel:{}", ch.id).as_bytes()),
        title: ch
            .name
            .clone(),
        kind: db::MediaKind::TvChannel,
        channel_number: ch
            .channel_number
            .map(|n| n as i64),
        external_ids: db::ExternalIds {
            iptv_source_id: Some(source_id.to_owned()),
            ..Default::default()
        },
        enabled: true,
        ..Default::default()
    }
}

pub(crate) fn stream_to_media(
    stream: &DispatcharrStream,
    channel_media_id: Uuid,
    idx: i64,
    playback_id: &str,
    base_url: &str,
    api_key: &str,
    now: chrono::NaiveDateTime,
) -> db::Media {
    let title = stream_title(stream);
    // No trailing slash: `/proxy/ts/stream/<id>/` falls through to
    // Dispatcharr's SPA catch-all and returns HTML, not the stream.
    db::Media {
        id: Uuid::new_v5(
            &channel_media_id,
            format!("stream:{}", stream.id).as_bytes(),
        ),
        title,
        kind: db::MediaKind::Stream,
        parent_id: Some(channel_media_id),
        idx: Some(idx),
        // Explicit, not the constructor-time default: this must be >= the
        // parent's `streams_refreshed_at` marker set right after this upsert,
        // or `.streams()`'s freshness filter (`updated_at >= refreshed`)
        // would immediately hide every version just synced.
        created_at: now,
        updated_at: now,
        stream_info: Some(crate::stream::StreamInfo {
            descriptor: crate::stream::StreamDescriptor::Http {
                url: format!("{base_url}/proxy/ts/stream/{playback_id}"),
                request_headers: std::collections::HashMap::from([(
                    "X-API-Key".to_string(),
                    api_key.to_string(),
                )]),
                response_headers: Default::default(),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Id a version pins one specific stream by: its `stream_hash`, falling back
/// to the numeric id. Resolved by Dispatcharr's `/proxy/ts/stream/<id>`
/// (`get_stream_object()` accepts a channel UUID or a stream hash).
fn pinned_playback_id(stream: &DispatcharrStream) -> String {
    stream
        .stream_hash
        .clone()
        .unwrap_or_else(|| {
            stream
                .id
                .to_string()
        })
}

/// The selectable versions of one channel, in `idx` order.
///
/// Version 0 plays via Dispatcharr's own channel-level proxy (channel UUID),
/// so Dispatcharr picks the stream, failover included. Every stream also gets
/// a version pinned to it by hash. For a channel with several streams that
/// includes the first one: without its own pinned row the first stream could
/// not be forced when Dispatcharr routes the default elsewhere. A channel with
/// a single stream has nothing to choose between, so it keeps just the default.
pub(crate) fn channel_versions(
    channel: &DispatcharrChannel,
    streams: &[DispatcharrStream],
    channel_media_id: Uuid,
    base_url: &str,
    api_key: &str,
    now: chrono::NaiveDateTime,
) -> Vec<db::Media> {
    let mut rows = Vec::with_capacity(streams.len() + 1);
    for (i, stream) in streams
        .iter()
        .enumerate()
    {
        if i == 0 {
            rows.push(stream_to_media(
                stream,
                channel_media_id,
                0,
                &channel.uuid,
                base_url,
                api_key,
                now,
            ));
            if streams.len() > 1 {
                let mut pinned = stream_to_media(
                    stream,
                    channel_media_id,
                    1,
                    &pinned_playback_id(stream),
                    base_url,
                    api_key,
                    now,
                );
                pinned.id = Uuid::new_v5(
                    &channel_media_id,
                    format!("pinned:{}", stream.id).as_bytes(),
                );
                rows.push(pinned);
            }
        } else {
            rows.push(stream_to_media(
                stream,
                channel_media_id,
                i as i64 + 1,
                &pinned_playback_id(stream),
                base_url,
                api_key,
                now,
            ));
        }
    }
    rows
}

/// Id of the synced `Recording` item for one Dispatcharr recording. Derived,
/// not stored, so it is known before the row is synced.
pub(crate) fn recording_media_id(addon_id: Uuid, recording_id: i64) -> Uuid {
    Uuid::new_v5(&addon_id, format!("recording:{recording_id}").as_bytes())
}

/// Builds the parent `Recording` row for one Dispatcharr DVR recording —
/// same shape `channel_to_media` uses for channels. The playable file itself
/// is a separate `Stream`-kind child (`recording_stream_to_media`), never
/// this row's own `stream_info`.
pub(crate) fn recording_to_media(
    rec: &super::dispatcharr_dvr::DispatcharrRecording,
    addon_id: Uuid,
    source_id: &str,
) -> db::Media {
    let id = recording_media_id(addon_id, rec.id);
    db::Media {
        id,
        title: rec
            .program_title()
            .unwrap_or("Recording")
            .to_string(),
        kind: db::MediaKind::Recording,
        description: rec
            .program_description()
            .map(str::to_owned),
        live_start: Some(
            rec.start_time
                .naive_utc(),
        ),
        live_end: Some(
            rec.end_time
                .naive_utc(),
        ),
        runtime: Some(
            (rec.end_time - rec.start_time)
                .num_seconds()
                .max(0),
        ),
        parent_id: Some(Uuid::new_v5(
            &addon_id,
            format!("channel:{}", rec.channel).as_bytes(),
        )),
        external_ids: db::ExternalIds {
            iptv_source_id: Some(source_id.to_owned()),
            dispatcharr_recording_id: Some(rec.id),
            ..Default::default()
        },
        enabled: true,
        ..Default::default()
    }
}

/// Loopback URL of a recording's own playback endpoint
/// (`GET /livetv/liverecordings/{id}/stream`), the address the internal
/// ffprobe pass and `/videos/{id}/stream` read a recording through.
///
/// Dispatcharr's own `/file/` redirects an in-progress recording to a playlist
/// whose segments need `X-API-Key`, which ffprobe cannot send; this endpoint
/// rewrites them to route back through remux.
pub(crate) fn recording_stream_url(port: u16, recording_media_id: Uuid) -> String {
    format!("http://127.0.0.1:{port}/livetv/liverecordings/{recording_media_id}/stream")
}

/// The single playable `Stream`-kind child of a synced `Recording` row.
/// `stream_info` points at remux's own recording endpoint
/// (`recording_stream_url`), which attaches Dispatcharr's `X-API-Key`
/// server-side, so no credentials are stored on the row.
pub(crate) fn recording_stream_to_media(
    rec: &super::dispatcharr_dvr::DispatcharrRecording,
    recording_media_id: Uuid,
    stream_url: &str,
    now: chrono::NaiveDateTime,
) -> db::Media {
    db::Media {
        id: Uuid::new_v5(&recording_media_id, b"stream"),
        title: rec
            .program_title()
            .unwrap_or("Recording")
            .to_string(),
        kind: db::MediaKind::Stream,
        parent_id: Some(recording_media_id),
        idx: Some(0),
        created_at: now,
        updated_at: now,
        stream_info: Some(crate::stream::StreamInfo {
            descriptor: crate::stream::StreamDescriptor::http(stream_url),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Runtime addon
// ---------------------------------------------------------------------------

pub(crate) struct DispatcharrAddon {
    pub(crate) addon_id: Uuid,
    pub(crate) base_url: String,
    pub(crate) api_key: String,
}

impl DispatcharrAddon {
    pub(crate) fn source_id(&self) -> String {
        self.addon_id
            .simple()
            .to_string()
    }
}

#[async_trait]
impl AddonKind for DispatcharrAddon {
    fn id(&self) -> &'static str {
        "dispatcharr"
    }

    async fn available_info(
        &self,
    ) -> Result<Option<(Vec<remux_sdks::stremio::ResourceRef>, Vec<StremioMediaType>)>>
    {
        let make_ref = |name| remux_sdks::stremio::ResourceRef {
            name,
            types: vec![],
            id_prefixes: None,
        };
        Ok(Some((
            vec![
                make_ref(ResourceType::Stream),
                make_ref(ResourceType::Catalog),
            ],
            vec![StremioMediaType::Tv],
        )))
    }
}

#[async_trait]
impl CatalogAddon for DispatcharrAddon {
    async fn catalog_list(&self, _ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        Ok(vec![CatalogInfo {
            provider_catalog_id: "channels".to_string(),
            name: "Dispatcharr Channels".to_string(),
            default_enabled: true,
            default_max_items: Some(999999999),
            collection_media_kind: None,
            media_kind: Some(db::MediaKind::TvChannel),
        }])
    }

    async fn catalog_stream(
        &self,
        _ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        if local_id != "channels" {
            return Ok(None);
        }

        let client = CLIENT.clone();
        let source_id = self.source_id();
        let addon_id = self.addon_id;

        debug!(base_url = %self.base_url, "fetching Dispatcharr channels");
        let channels = fetch_channels(&client, &self.base_url, &self.api_key).await?;
        let items: Vec<db::Media> = channels
            .iter()
            .map(|ch| channel_to_media(ch, addon_id, &source_id))
            .collect();

        Ok(Some(Box::pin(futures::stream::iter(items))))
    }
}

#[async_trait]
impl StreamAddon for DispatcharrAddon {
    fn supports(&self, media: &db::Media) -> bool {
        media.kind == db::MediaKind::TvChannel
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
        _id_prefixes: Option<&[String]>,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        // Channel versions come from the synced `Stream` children, not from
        // this trait method; echo the row, as `IptvAddon` does.
        let Some(ref si) = media.stream_info else {
            return Ok(vec![]);
        };
        Ok(vec![si.clone()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::dispatcharr_dvr::DispatcharrRecording;
    use serde_json::json;

    const ADDON: Uuid = Uuid::from_u128(0xd15b);

    fn now() -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 9, 15)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn channel(v: serde_json::Value) -> DispatcharrChannel {
        serde_json::from_value(v).unwrap()
    }

    fn stream(v: serde_json::Value) -> DispatcharrStream {
        serde_json::from_value(v).unwrap()
    }

    fn recording(v: serde_json::Value) -> DispatcharrRecording {
        serde_json::from_value(v).unwrap()
    }

    fn sample_recording() -> DispatcharrRecording {
        recording(json!({
            "id": 5,
            "channel": 42,
            "start_time": "2026-09-15T20:00:00Z",
            "end_time": "2026-09-15T21:30:00Z",
            "custom_properties": {
                "status": "completed",
                "program": { "title": "Match of the Day", "description": "Highlights." }
            }
        }))
    }

    fn http_parts(
        media: &db::Media,
    ) -> (&str, &std::collections::HashMap<String, String>) {
        match &media
            .stream_info
            .as_ref()
            .unwrap()
            .descriptor
        {
            crate::stream::StreamDescriptor::Http {
                url,
                request_headers,
                ..
            } => (url, request_headers),
            other => panic!("expected an Http descriptor, got {other:?}"),
        }
    }

    // -- stream labels ------------------------------------------------

    /// Shape of a real Dispatcharr `stream_stats` blob: `resolution` is a
    /// "WxH" string, and the height and frame rate are top-level keys.
    fn stats(height: i64, fps: f64) -> serde_json::Value {
        json!({
            "width": height * 16 / 9,
            "height": height,
            "resolution": format!("{}x{}", height * 16 / 9, height),
            "source_fps": fps,
            "video_codec": "h264",
        })
    }

    fn titled(name: &str, stats: Option<serde_json::Value>) -> String {
        stream_title(&stream(json!({
            "id": 1, "name": name, "stream_stats": stats,
        })))
    }

    #[test]
    fn resolution_label_buckets_by_height() {
        let label = |h| resolution_label(&stats(h, 25.0));
        for (height, want) in [
            (2160, "4K"),
            (2000, "4K"),
            (1999, "FHD"),
            (1080, "FHD"),
            (1000, "FHD"),
            (999, "HD"),
            (720, "HD"),
            (700, "HD"),
            (699, "SD"),
            (576, "SD"),
            (540, "SD"),
        ] {
            assert_eq!(label(height), Some(want), "{height}p");
        }
    }

    #[test]
    fn resolution_label_prefers_nested_resolution_height() {
        let stats = json!({ "height": 1080, "resolution": { "height": 2160 } });
        assert_eq!(resolution_label(&stats), Some("4K"));
    }

    #[test]
    fn resolution_label_is_none_without_a_numeric_height() {
        assert_eq!(resolution_label(&json!({})), None);
        assert_eq!(resolution_label(&json!({ "height": null })), None);
        // The "WxH" string alone is not parsed; only a numeric height counts.
        assert_eq!(
            resolution_label(&json!({ "resolution": "1920x1080" })),
            None
        );
    }

    #[test]
    fn fps_label_rounds_to_whole_frames() {
        let label = |f| fps_label(&stats(1080, f));
        assert_eq!(label(50.0).as_deref(), Some("50fps"));
        assert_eq!(label(25.0).as_deref(), Some("25fps"));
        assert_eq!(label(23.976).as_deref(), Some("24fps"));
        assert_eq!(label(29.97).as_deref(), Some("30fps"));
        assert_eq!(label(59.94).as_deref(), Some("60fps"));
    }

    #[test]
    fn fps_label_is_none_without_a_positive_rate() {
        assert_eq!(fps_label(&json!({})), None);
        assert_eq!(fps_label(&json!({ "source_fps": null })), None);
        assert_eq!(fps_label(&json!({ "source_fps": 0 })), None);
        assert_eq!(fps_label(&json!({ "source_fps": -25.0 })), None);
        assert_eq!(fps_label(&json!({ "source_fps": "50" })), None);
    }

    #[test]
    fn superscripts_fold_to_ascii() {
        // Ends of each block in the table, so a misaligned entry shows up.
        for (from, to) in [
            ('ᴬ', 'a'),
            ('ᴰ', 'd'),
            ('ᴴ', 'h'),
            ('ᴿ', 'r'),
            ('ᵁ', 'u'),
            ('ᵂ', 'w'),
            ('ᵃ', 'a'),
            ('ᶠ', 'f'),
            ('ᵖ', 'p'),
            ('ʳ', 'r'),
            ('ˢ', 's'),
            ('ᶻ', 'z'),
            ('⁰', '0'),
            ('⁴', '4'),
            ('⁹', '9'),
            ('ᴷ', 'k'),
        ] {
            assert_eq!(fold_superscript(from), to, "{from}");
        }
        assert_eq!(fold_superscript('x'), 'x');
        assert_eq!(fold_superscript('Z'), 'Z');
        assert_eq!(fold_superscript('7'), '7');
    }

    #[test]
    fn clean_stream_name_handles_real_provider_names() {
        for (raw, want) in [
            ("ES: LA 2 ᵁᴴᴰ", "LA 2"),
            ("ES: LA 2 SD", "LA 2"),
            ("ES: LA 2 ᴴᴰ", "LA 2"),
            ("GO: LA 2 ᴿᴬᵂ", "LA 2"),
            ("AV: LA2 ᴿᴬᵂ", "LA2"),
            ("TV: LA 2 ᴿᴬᵂ", "LA 2"),
            ("VO: LA 2 ᴴᴰ", "LA 2"),
        ] {
            assert_eq!(clean_stream_name(raw), want, "{raw}");
        }
    }

    #[test]
    fn clean_stream_name_strips_other_prefix_and_quality_styles() {
        for (raw, want) in [
            ("|UK| Sky Sports Main Event 4K", "Sky Sports Main Event"),
            ("UK | BBC One FHD 50FPS", "BBC One"),
            ("BBC One HD", "BBC One"),
            ("Sky Cinema (HD)", "Sky Cinema"),
            ("Eurosport 1 1080p50", "Eurosport 1"),
            ("Discovery ⁶⁰ᶠᵖˢ", "Discovery"),
            ("⁴ᴷ Movies", "Movies"),
            ("  ES:   LA 2   ᴴᴰ  ", "LA 2"),
        ] {
            assert_eq!(clean_stream_name(raw), want, "{raw}");
        }
    }

    #[test]
    fn clean_stream_name_leaves_ordinary_names_alone() {
        for name in [
            "Channel 4",
            "Sky Sports F1",
            "CBBC",
            "Sky: News",
            "Movistar Plus+ 1",
        ] {
            assert_eq!(clean_stream_name(name), name);
        }
    }

    #[test]
    fn clean_stream_name_falls_back_to_the_raw_name_when_nothing_is_left() {
        assert_eq!(clean_stream_name("HD"), "HD");
        assert_eq!(clean_stream_name(" ES: ᴴᴰ "), "ES: ᴴᴰ");
    }

    #[test]
    fn stream_title_reads_resolution_then_fps_then_name() {
        assert_eq!(
            titled("ES: LA 2 ᵁᴴᴰ", Some(stats(1080, 50.0))),
            "FHD · 50fps · LA 2"
        );
        assert_eq!(
            titled("VO: LA 2 ᴴᴰ", Some(stats(720, 25.0))),
            "HD · 25fps · LA 2"
        );
        assert_eq!(
            titled("Sky Sports 4K", Some(stats(2160, 50.0))),
            "4K · 50fps · Sky Sports"
        );
        assert_eq!(
            titled("Old Channel", Some(stats(576, 25.0))),
            "SD · 25fps · Old Channel"
        );
    }

    #[test]
    fn stream_title_follows_the_probe_not_the_name() {
        // Providers mislabel: this "SD" stream probes as 1080p.
        assert_eq!(
            titled("ES: LA 2 SD", Some(stats(1080, 30.0))),
            "FHD · 30fps · LA 2"
        );
    }

    #[test]
    fn stream_title_omits_what_was_not_probed() {
        assert_eq!(titled("ES: LA 2 ᴴᴰ", None), "LA 2");
        assert_eq!(
            titled("LA 2", Some(json!({ "height": 1080 }))),
            "FHD · LA 2"
        );
        assert_eq!(
            titled("LA 2", Some(json!({ "source_fps": 50.0 }))),
            "50fps · LA 2"
        );
    }

    // -- channel_to_media ---------------------------------------------

    #[test]
    fn channel_to_media_maps_fields() {
        let ch = channel(json!({
            "id": 42, "uuid": "u-42", "name": "BBC One", "channel_number": 1.0,
        }));
        let m = channel_to_media(&ch, ADDON, "src");
        assert_eq!(m.title, "BBC One");
        assert_eq!(m.kind, db::MediaKind::TvChannel);
        assert_eq!(m.channel_number, Some(1));
        assert_eq!(
            m.external_ids
                .iptv_source_id
                .as_deref(),
            Some("src")
        );
        assert!(m.enabled);
    }

    #[test]
    fn channel_to_media_does_not_copy_the_channels_own_tvg_id() {
        // EPG is matched through `epg_data_id`; the channel's own `tvg_id` is a
        // copy that can go stale, so it is not carried onto the row.
        let ch = channel(json!({
            "id": 42, "uuid": "u", "name": "BBC One", "tvg_id": "BBC1.uk",
        }));
        assert_eq!(channel_to_media(&ch, ADDON, "src").tvg_id, None);
    }

    #[test]
    fn channel_to_media_id_is_stable_and_scoped_to_addon() {
        let ch = channel(json!({ "id": 42, "uuid": "u", "name": "A" }));
        let other = channel(json!({ "id": 43, "uuid": "u", "name": "A" }));
        let a = channel_to_media(&ch, ADDON, "s").id;
        assert_eq!(a, channel_to_media(&ch, ADDON, "s").id);
        assert_eq!(a, Uuid::new_v5(&ADDON, b"channel:42"));
        assert_ne!(a, channel_to_media(&other, ADDON, "s").id);
        assert_ne!(a, channel_to_media(&ch, Uuid::from_u128(1), "s").id);
    }

    #[test]
    fn channel_to_media_truncates_fractional_channel_numbers() {
        let ch = channel(json!({
            "id": 1, "uuid": "u", "name": "A", "channel_number": 7.5,
        }));
        assert_eq!(channel_to_media(&ch, ADDON, "s").channel_number, Some(7));

        let bare = channel(json!({ "id": 2, "uuid": "u", "name": "B" }));
        assert_eq!(channel_to_media(&bare, ADDON, "s").channel_number, None);
    }

    #[test]
    fn channel_deserialises_epg_data_id_and_tolerates_absence() {
        let with =
            channel(json!({ "id": 1, "uuid": "u", "name": "A", "epg_data_id": 9 }));
        assert_eq!(with.epg_data_id, Some(9));
        let without = channel(json!({ "id": 1, "uuid": "u", "name": "A" }));
        assert_eq!(without.epg_data_id, None);
        assert_eq!(without.logo_id, None);
    }

    // -- stream_to_media ----------------------------------------------

    #[test]
    fn stream_to_media_uses_the_stream_title() {
        let s = stream(json!({
            "id": 7, "name": "ES: LA 2 SD", "stream_hash": "h",
            "stream_stats": { "height": 540, "resolution": "960x540", "source_fps": 25.0 },
        }));
        let m = stream_to_media(&s, Uuid::from_u128(1), 0, "p", "http://d", "k", now());
        assert_eq!(m.title, "SD · 25fps · LA 2");

        let unprobed = stream(json!({ "id": 8, "name": "Raw", "stream_hash": null }));
        let m = stream_to_media(
            &unprobed,
            Uuid::from_u128(1),
            0,
            "p",
            "http://d",
            "k",
            now(),
        );
        assert_eq!(m.title, "Raw");
    }

    #[test]
    fn stream_to_media_attaches_to_channel_in_order() {
        let s = stream(json!({ "id": 7, "name": "S" }));
        let parent = Uuid::from_u128(1);
        let m = stream_to_media(&s, parent, 3, "p", "http://d", "k", now());
        assert_eq!(m.kind, db::MediaKind::Stream);
        assert_eq!(m.parent_id, Some(parent));
        assert_eq!(m.idx, Some(3));
        assert_eq!(m.created_at, now());
        assert_eq!(m.updated_at, now());
    }

    #[test]
    fn stream_to_media_id_depends_on_parent_and_stream() {
        let s = stream(json!({ "id": 7, "name": "S" }));
        let other = stream(json!({ "id": 8, "name": "S" }));
        let p1 = Uuid::from_u128(1);
        let id = |s: &DispatcharrStream, p| {
            stream_to_media(s, p, 0, "x", "http://d", "k", now()).id
        };
        assert_eq!(id(&s, p1), id(&s, p1));
        assert_eq!(id(&s, p1), Uuid::new_v5(&p1, b"stream:7"));
        assert_ne!(id(&s, p1), id(&other, p1));
        assert_ne!(id(&s, p1), id(&s, Uuid::from_u128(2)));
    }

    #[test]
    fn stream_to_media_points_at_the_proxy_with_the_api_key() {
        let s = stream(json!({ "id": 7, "name": "S" }));
        let m = stream_to_media(
            &s,
            Uuid::from_u128(1),
            0,
            "chan-uuid",
            "http://d:9191",
            "secret",
            now(),
        );
        let (url, headers) = http_parts(&m);
        // A trailing slash falls through to Dispatcharr's SPA catch-all.
        assert_eq!(url, "http://d:9191/proxy/ts/stream/chan-uuid");
        assert_eq!(
            headers
                .get("X-API-Key")
                .map(String::as_str),
            Some("secret")
        );
        assert_eq!(headers.len(), 1);
    }

    // -- recordings ---------------------------------------------------

    #[test]
    fn recording_to_media_maps_fields() {
        let m = recording_to_media(&sample_recording(), ADDON, "src");
        assert_eq!(m.kind, db::MediaKind::Recording);
        assert_eq!(m.title, "Match of the Day");
        assert_eq!(
            m.description
                .as_deref(),
            Some("Highlights.")
        );
        assert_eq!(m.runtime, Some(90 * 60));
        assert_eq!(
            m.live_start
                .unwrap()
                .to_string(),
            "2026-09-15 20:00:00"
        );
        assert_eq!(
            m.live_end
                .unwrap()
                .to_string(),
            "2026-09-15 21:30:00"
        );
        assert_eq!(
            m.external_ids
                .dispatcharr_recording_id,
            Some(5)
        );
        assert_eq!(
            m.external_ids
                .iptv_source_id
                .as_deref(),
            Some("src")
        );
        assert_eq!(m.id, Uuid::new_v5(&ADDON, b"recording:5"));
    }

    #[test]
    fn recording_is_parented_to_the_synced_channel_row() {
        // The FK only holds if both sides derive the id the same way.
        let ch = channel(json!({ "id": 42, "uuid": "u", "name": "BBC One" }));
        let channel_row = channel_to_media(&ch, ADDON, "src");
        let rec = recording_to_media(&sample_recording(), ADDON, "src");
        assert_eq!(rec.parent_id, Some(channel_row.id));
    }

    #[test]
    fn recording_title_falls_back_when_unnamed() {
        let rec = recording(json!({
            "id": 1, "channel": 1,
            "start_time": "2026-09-15T20:00:00Z",
            "end_time": "2026-09-15T21:00:00Z",
        }));
        let m = recording_to_media(&rec, ADDON, "s");
        assert_eq!(m.title, "Recording");
        assert_eq!(m.description, None);
    }

    #[test]
    fn recording_runtime_never_goes_negative() {
        let rec = recording(json!({
            "id": 1, "channel": 1,
            "start_time": "2026-09-15T21:00:00Z",
            "end_time": "2026-09-15T20:00:00Z",
        }));
        assert_eq!(recording_to_media(&rec, ADDON, "s").runtime, Some(0));
    }

    #[test]
    fn recording_stream_points_at_remuxs_own_endpoint() {
        let rec = sample_recording();
        let parent = recording_to_media(&rec, ADDON, "s");
        let url = recording_stream_url(3000, parent.id);
        let m = recording_stream_to_media(&rec, parent.id, &url, now());
        assert_eq!(m.kind, db::MediaKind::Stream);
        assert_eq!(m.parent_id, Some(parent.id));
        assert_eq!(m.idx, Some(0));
        assert_eq!(m.id, Uuid::new_v5(&parent.id, b"stream"));
        assert_eq!(m.title, "Match of the Day");
        let (got, headers) = http_parts(&m);
        assert_eq!(
            got,
            format!(
                "http://127.0.0.1:3000/livetv/liverecordings/{}/stream",
                parent.id
            )
        );
        // No Dispatcharr credentials are stored on the row.
        assert!(headers.is_empty());
    }

    // -- channel versions ---------------------------------------------

    fn versions(n: usize) -> Vec<db::Media> {
        let ch = channel(json!({ "id": 42, "uuid": "chan-uuid", "name": "BBC" }));
        let streams: Vec<DispatcharrStream> = (0..n)
            .map(|i| {
                stream(json!({
                    "id": 100 + i, "name": format!("s{i}"),
                    "stream_hash": format!("hash{i}"),
                }))
            })
            .collect();
        channel_versions(&ch, &streams, Uuid::from_u128(1), "http://d", "k", now())
    }

    fn url_of(m: &db::Media) -> &str {
        http_parts(m).0
    }

    #[test]
    fn the_first_stream_gets_a_pinned_version_after_the_default() {
        let rows = versions(3);
        assert_eq!(
            rows.iter()
                .map(|m| m.idx)
                .collect::<Vec<_>>(),
            [Some(0), Some(1), Some(2), Some(3)]
        );
        assert_eq!(
            rows.iter()
                .map(url_of)
                .collect::<Vec<_>>(),
            [
                // default: Dispatcharr picks
                "http://d/proxy/ts/stream/chan-uuid",
                // stream 0, pinned
                "http://d/proxy/ts/stream/hash0",
                "http://d/proxy/ts/stream/hash1",
                "http://d/proxy/ts/stream/hash2",
            ]
        );
    }

    #[test]
    fn version_ids_are_distinct_and_the_established_ones_are_stable() {
        let rows = versions(3);
        let parent = Uuid::from_u128(1);
        let ids: std::collections::HashSet<_> = rows
            .iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids.len(), 4);
        // Same ids as before the pinned row existed.
        assert_eq!(rows[0].id, Uuid::new_v5(&parent, b"stream:100"));
        assert_eq!(rows[2].id, Uuid::new_v5(&parent, b"stream:101"));
        assert_eq!(rows[3].id, Uuid::new_v5(&parent, b"stream:102"));
        assert_eq!(rows[1].id, Uuid::new_v5(&parent, b"pinned:100"));
    }

    #[test]
    fn a_single_stream_channel_keeps_only_the_default() {
        let rows = versions(1);
        assert_eq!(rows.len(), 1);
        assert_eq!(url_of(&rows[0]), "http://d/proxy/ts/stream/chan-uuid");
        assert!(versions(0).is_empty());
    }

    #[test]
    fn a_stream_without_a_hash_is_pinned_by_its_id() {
        let ch = channel(json!({ "id": 42, "uuid": "chan-uuid", "name": "BBC" }));
        let streams = [
            stream(json!({ "id": 7, "name": "a" })),
            stream(json!({ "id": 8, "name": "b" })),
        ];
        let rows =
            channel_versions(&ch, &streams, Uuid::from_u128(1), "http://d", "k", now());
        assert_eq!(url_of(&rows[1]), "http://d/proxy/ts/stream/7");
        assert_eq!(url_of(&rows[2]), "http://d/proxy/ts/stream/8");
    }

    // -- preset -------------------------------------------------------

    #[test]
    fn preset_declares_the_two_required_options() {
        let meta = DispatcharrPreset.metadata();
        assert_eq!(DispatcharrPreset.id(), "dispatcharr");
        assert_eq!(meta.supported_types, vec![MediaKind::TvChannel]);
        let ids: Vec<&str> = meta
            .options
            .iter()
            .map(|o| {
                o.id.as_str()
            })
            .collect();
        assert_eq!(ids, ["base_url", "api_key"]);
        assert!(
            meta.options
                .iter()
                .all(|o| o.required)
        );
    }

    #[test]
    fn from_cfg_requires_base_url_and_api_key() {
        let cfg = crate::Config::default();
        let build =
            |v: serde_json::Value| DispatcharrPreset.from_cfg(Uuid::nil(), &v, &cfg);

        assert!(build(json!({ "base_url": "http://d", "api_key": "k" })).is_ok());
        for bad in [
            json!({}),
            json!({ "base_url": "http://d" }),
            json!({ "api_key": "k" }),
            json!({ "base_url": "", "api_key": "k" }),
            json!({ "base_url": "http://d", "api_key": "" }),
        ] {
            assert!(build(bad.clone()).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn from_cfg_error_names_the_missing_field() {
        let cfg = crate::Config::default();
        let err = DispatcharrPreset
            .from_cfg(Uuid::nil(), &json!({ "api_key": "k" }), &cfg)
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("base_url"), "{err}");
        let err = DispatcharrPreset
            .from_cfg(Uuid::nil(), &json!({ "base_url": "http://d" }), &cfg)
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("api_key"), "{err}");
    }

    #[test]
    fn addon_supports_only_tv_channels() {
        let addon = DispatcharrAddon {
            addon_id: ADDON,
            base_url: "http://d".into(),
            api_key: "k".into(),
        };
        let of = |kind| db::Media {
            kind,
            ..Default::default()
        };
        assert!(addon.supports(&of(db::MediaKind::TvChannel)));
        assert!(!addon.supports(&of(db::MediaKind::Movie)));
        assert!(!addon.supports(&of(db::MediaKind::Recording)));
    }

    #[test]
    fn source_id_is_the_simple_uuid() {
        let addon = DispatcharrAddon {
            addon_id: Uuid::from_u128(0xabc),
            base_url: String::new(),
            api_key: String::new(),
        };
        assert_eq!(addon.source_id(), "00000000000000000000000000000abc");
    }

    // -- HTTP client --------------------------------------------------

    #[tokio::test]
    async fn fetch_channels_sends_the_api_key_and_parses_the_array() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/channels/channels/")
                .header("X-API-Key", "secret");
            then.status(200)
                .json_body(json!([
                    { "id": 1, "uuid": "a", "name": "One", "epg_data_id": 10 },
                    { "id": 2, "uuid": "b", "name": "Two" },
                ]));
        });

        let got = fetch_channels(&reqwest::Client::new(), &server.base_url(), "secret")
            .await
            .unwrap();
        mock.assert();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].epg_data_id, Some(10));
        assert_eq!(got[1].epg_data_id, None);
    }

    #[tokio::test]
    async fn fetch_functions_surface_http_errors() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::GET);
            then.status(401)
                .body("nope");
        });
        let client = reqwest::Client::new();
        let base = server.base_url();
        assert!(
            fetch_channels(&client, &base, "bad")
                .await
                .is_err()
        );
        assert!(
            fetch_channel_streams(&client, &base, "bad", 1)
                .await
                .is_err()
        );
        assert!(
            fetch_epg_data(&client, &base, "bad")
                .await
                .is_err()
        );
        assert!(
            fetch_epg_grid(&client, &base, "bad")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn fetch_channel_streams_keeps_dispatcharrs_order() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/channels/channels/749/streams/")
                .header("X-API-Key", "k");
            then.status(200)
                .json_body(json!([
                    { "id": 30, "name": "first", "stream_hash": "h30" },
                    { "id": 10, "name": "second", "stream_hash": null,
                      "stream_stats": { "height": 1080 } },
                ]));
        });
        let got = fetch_channel_streams(
            &reqwest::Client::new(),
            &server.base_url(),
            "k",
            749,
        )
        .await
        .unwrap();
        let ids: Vec<i64> = got
            .iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, [30, 10]);
        assert_eq!(
            got[0]
                .stream_hash
                .as_deref(),
            Some("h30")
        );
        assert_eq!(got[1].stream_hash, None);
    }

    #[tokio::test]
    async fn fetch_epg_data_parses_tvg_ids() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/epg/epgdata/")
                .header("X-API-Key", "k");
            then.status(200)
                .json_body(json!([
                    { "id": 10, "tvg_id": "BBC1.uk" },
                    { "id": 11, "tvg_id": null },
                ]));
        });
        let got = fetch_epg_data(&reqwest::Client::new(), &server.base_url(), "k")
            .await
            .unwrap();
        assert_eq!(
            got[0]
                .tvg_id
                .as_deref(),
            Some("BBC1.uk")
        );
        assert_eq!(got[1].tvg_id, None);
    }

    #[tokio::test]
    async fn fetch_epg_grid_unwraps_the_data_envelope() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/epg/grid/")
                .header("X-API-Key", "k");
            then.status(200)
                .json_body(json!({ "data": [{
                    "start_time": "2026-09-15T20:00:00Z",
                    "end_time": "2026-09-15T21:00:00Z",
                    "title": "News", "sub_title": "Late", "tvg_id": "BBC1.uk",
                }]}));
        });
        let got = fetch_epg_grid(&reqwest::Client::new(), &server.base_url(), "k")
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "News");
        assert_eq!(
            got[0]
                .sub_title
                .as_deref(),
            Some("Late")
        );
        assert_eq!(got[0].description, None);
        assert_eq!(got[0].tvg_id, "BBC1.uk");
    }

    #[tokio::test]
    async fn catalog_stream_lists_channels_through_a_configured_addon() {
        use crate::integration_test::new_test_server;
        use futures::StreamExt;

        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/channels/channels/")
                .header("X-API-Key", "k");
            then.status(200)
                .json_body(json!([
                    { "id": 1, "uuid": "a", "name": "One" },
                    { "id": 2, "uuid": "b", "name": "Two" },
                ]));
        });

        let (_server, guard) = new_test_server()
            .await
            .unwrap();
        // Trailing slashes on the configured URL must not double up in requests.
        let caps = DispatcharrPreset
            .from_cfg(
                ADDON,
                &json!({ "base_url": format!("{}//", server.base_url()), "api_key": "k" }),
                &crate::Config::default(),
            )
            .unwrap();
        let catalog = caps
            .catalog
            .expect("catalog capability");

        let listed = catalog
            .catalog_list(&guard.0)
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].provider_catalog_id, "channels");

        let items: Vec<db::Media> = catalog
            .catalog_stream(&guard.0, "channels")
            .await
            .unwrap()
            .expect("channels catalog")
            .collect()
            .await;
        let titles: Vec<&str> = items
            .iter()
            .map(|m| {
                m.title
                    .as_str()
            })
            .collect();
        assert_eq!(titles, ["One", "Two"]);
        assert!(
            items
                .iter()
                .all(|m| m.kind == db::MediaKind::TvChannel)
        );

        assert!(
            catalog
                .catalog_stream(&guard.0, "something-else")
                .await
                .unwrap()
                .is_none()
        );
    }
}
