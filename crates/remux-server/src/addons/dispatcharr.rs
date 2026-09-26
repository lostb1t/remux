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
                AddonOption {
                    id: EPG_LOOKBACK_OPTION.to_string(),
                    name: "Guide hours behind".to_string(),
                    description: Some(
                        "How many hours of already-aired programmes to import into the guide."
                            .to_string(),
                    ),
                    required: false,
                    default: Some(serde_json::json!(EpgWindow::DEFAULT.lookback_hours)),
                    kind: AddonOptionType::Number {
                        min: Some(0),
                        max: Some(EpgWindow::MAX_LOOKBACK_HOURS),
                    },
                },
                AddonOption {
                    id: EPG_LOOKAHEAD_OPTION.to_string(),
                    name: "Guide hours ahead".to_string(),
                    description: Some(
                        "How many hours of upcoming programmes to import into the guide. \
                         Keep it longer than the refresh interval (12 hours by default) or \
                         the guide runs out between refreshes."
                            .to_string(),
                    ),
                    required: false,
                    default: Some(serde_json::json!(EpgWindow::DEFAULT.lookahead_hours)),
                    kind: AddonOptionType::Number {
                        min: Some(1),
                        max: Some(EpgWindow::MAX_LOOKAHEAD_HOURS),
                    },
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
    /// FK to the `EPGData` row of this channel's assigned guide. EPG is
    /// resolved through this, not the channel's own `tvg_id`, which can go
    /// stale.
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

/// HTTP client, with timeouts, for every Dispatcharr API call.
pub(crate) static CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent("remux-server/1.0")
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build Dispatcharr client")
    });

/// Fetch every Dispatcharr channel. Without `page_size` the endpoint returns a
/// plain array rather than DRF's `{count,results}` envelope.
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

/// Fetch the streams assigned to one channel, in Dispatcharr's priority order.
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
    /// The EPG source this row belongs to. `tvg_id` is only unique within
    /// one source, so this is what tells two sources' same-named rows apart.
    #[serde(default)]
    pub epg_source: Option<i64>,
}

/// Fetch every `EPGData` row, the per-source guide a `Channel.epg_data_id`
/// points at.
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
    /// Dispatcharr's id for the programme, which its recordings reference: a
    /// number for guide data, a string for generated placeholder programmes.
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    /// Nullable in Dispatcharr's grid schema; a programme without one is
    /// skipped rather than failing the whole response.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub sub_title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tvg_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EpgGridResponse {
    data: Vec<DispatcharrProgram>,
}

/// Addon option ids for the guide window.
const EPG_LOOKBACK_OPTION: &str = "epg_lookback_hours";
const EPG_LOOKAHEAD_OPTION: &str = "epg_lookahead_hours";

/// How much of the guide to import around now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EpgWindow {
    pub lookback_hours: i64,
    pub lookahead_hours: i64,
}

impl EpgWindow {
    pub const DEFAULT: Self = Self {
        lookback_hours: 2,
        lookahead_hours: 72,
    };
    /// Dispatcharr caps its own relative lookback at 30 days.
    pub const MAX_LOOKBACK_HOURS: i64 = 30 * 24;
    pub const MAX_LOOKAHEAD_HOURS: i64 = 365 * 24;

    /// The window an addon's config asks for. A missing or non-numeric value
    /// takes the default; an out-of-range one is clamped.
    pub fn from_cfg(cfg: &serde_json::Value) -> Self {
        let hours = |key: &str, default: i64, min: i64, max: i64| {
            cfg.get(key)
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| {
                            v.as_str()
                                .and_then(|s| {
                                    s.trim()
                                        .parse()
                                        .ok()
                                })
                        })
                })
                .unwrap_or(default)
                .clamp(min, max)
        };
        Self {
            lookback_hours: hours(
                EPG_LOOKBACK_OPTION,
                Self::DEFAULT.lookback_hours,
                0,
                Self::MAX_LOOKBACK_HOURS,
            ),
            lookahead_hours: hours(
                EPG_LOOKAHEAD_OPTION,
                Self::DEFAULT.lookahead_hours,
                1,
                Self::MAX_LOOKAHEAD_HOURS,
            ),
        }
    }

    /// `(start, end)` of the window around `now`.
    pub fn bounds(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
        (
            now - chrono::Duration::hours(self.lookback_hours),
            now + chrono::Duration::hours(self.lookahead_hours),
        )
    }
}

/// Fetch every channel's programmes overlapping `start..end`.
///
/// Programmes are keyed by `EPGData.tvg_id`; map a channel to it through its
/// `epg_data_id` and `fetch_epg_data()`.
pub(crate) async fn fetch_epg_grid(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<DispatcharrProgram>> {
    let resp = client
        .get(format!(
            "{base_url}/api/epg/grid/?start={}&end={}",
            urlencoding::encode(&start.to_rfc3339()),
            urlencoding::encode(&end.to_rfc3339()),
        ))
        .header("X-API-Key", token)
        .send()
        .await?
        .error_for_status()?
        .json::<EpgGridResponse>()
        .await?;
    Ok(resp.data)
}

#[derive(Debug, Deserialize)]
struct ProgramSearchPage {
    results: Vec<DispatcharrProgram>,
    #[serde(default)]
    next: Option<String>,
}

/// Programmes of one `(tvg_id, epg_source)` guide overlapping `start..end`.
///
/// The grid tags programmes with `tvg_id` alone, which cannot tell apart two
/// sources that use the same id; this asks for one source's copy by name.
pub(crate) async fn fetch_source_programs(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    tvg_id: &str,
    epg_source: i64,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<DispatcharrProgram>> {
    let mut programs = Vec::new();
    for page in 1.. {
        let resp = client
            .get(format!(
                "{base_url}/api/epg/programs/search/?tvg_id={}&epg_source={epg_source}\
                 &end_after={}&start_before={}&page_size=500&page={page}",
                urlencoding::encode(tvg_id),
                urlencoding::encode(&start.to_rfc3339()),
                urlencoding::encode(&end.to_rfc3339()),
            ))
            .header("X-API-Key", token)
            .send()
            .await?
            .error_for_status()?
            .json::<ProgramSearchPage>()
            .await?;
        programs.extend(resp.results);
        if resp
            .next
            .is_none()
        {
            break;
        }
    }
    Ok(programs)
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

/// Words in a stream name that describe quality or encoding rather than the
/// channel: `HD`, `FHD`, `4K`, `1080p`, `1080p50`, `50fps`, `RAW`, `HEVC`,
/// `H265`, `HDR`.
static QUALITY_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:sd|hd|fhd|uhd|qhd|4k|8k|raw|\d{3,4}[pi]\d{0,3}|\d{2,3}fps|hevc|[hx]\.?26[45]|hdr(?:10)?\+?)$",
    )
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
    db::Media {
        id: Uuid::new_v5(
            &channel_media_id,
            format!("stream:{}", stream.id).as_bytes(),
        ),
        title,
        kind: db::MediaKind::Stream,
        parent_id: Some(channel_media_id),
        idx: Some(idx),
        // Overwritten by `Media::upsert`; set for callers that use the row
        // without upserting it.
        created_at: now,
        updated_at: now,
        stream_info: Some(crate::stream::StreamInfo {
            descriptor: crate::stream::StreamDescriptor::Http {
                // No trailing slash: Dispatcharr's SPA catch-all answers that
                // with HTML.
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

/// The selectable versions of one channel, in `idx` order.
///
/// Version 0 plays through Dispatcharr's channel-level proxy, which picks the
/// stream and fails over. A channel with more than one stream also gets a
/// version pinned to each stream that has a `stream_hash`.
pub(crate) fn channel_versions(
    channel: &DispatcharrChannel,
    streams: &[DispatcharrStream],
    channel_media_id: Uuid,
    base_url: &str,
    api_key: &str,
    now: chrono::NaiveDateTime,
) -> Vec<db::Media> {
    let mut rows = Vec::with_capacity(streams.len() + 1);
    if let Some(first) = streams.first() {
        rows.push(stream_to_media(
            first,
            channel_media_id,
            0,
            &channel.uuid,
            base_url,
            api_key,
            now,
        ));
    }
    if streams.len() < 2 {
        return rows;
    }
    for (i, stream) in streams
        .iter()
        .enumerate()
    {
        let Some(hash) = stream
            .stream_hash
            .as_deref()
        else {
            continue;
        };
        let mut pinned = stream_to_media(
            stream,
            channel_media_id,
            i as i64 + 1,
            hash,
            base_url,
            api_key,
            now,
        );
        if i == 0 {
            // `stream:{id}` is already the default's id.
            pinned.id = Uuid::new_v5(
                &channel_media_id,
                format!("pinned:{}", stream.id).as_bytes(),
            );
        }
        rows.push(pinned);
    }
    rows
}

/// A Dispatcharr programme id as text, whichever JSON type it came as.
pub(crate) fn program_key(id: &serde_json::Value) -> Option<String> {
    match id {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// `program_key` back as the JSON value Dispatcharr uses for it.
pub(crate) fn program_key_value(key: &str) -> serde_json::Value {
    key.parse::<i64>()
        .map(serde_json::Value::from)
        .unwrap_or_else(|_| serde_json::Value::from(key))
}

/// Id of the synced guide row for one programme on one channel.
pub(crate) fn program_media_id(
    channel_id: Uuid,
    start: chrono::DateTime<chrono::Utc>,
    title: &str,
) -> Uuid {
    Uuid::new_v5(&channel_id, format!("{start}{title}").as_bytes())
}

/// Id of the synced `Recording` item for one Dispatcharr recording.
pub(crate) fn recording_media_id(addon_id: Uuid, recording_id: i64) -> Uuid {
    Uuid::new_v5(&addon_id, format!("recording:{recording_id}").as_bytes())
}

/// Builds the `Recording` row for one Dispatcharr DVR recording. Its playable
/// file is the `Stream` child built by `recording_stream_to_media`.
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
            rec.actual_end()
                .naive_utc(),
        ),
        runtime: Some(
            (rec.actual_end() - rec.start_time)
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

/// Loopback URL of `GET /livetv/liverecordings/{id}/stream`, which ffprobe and
/// `/videos/{id}/stream` read a recording through.
pub(crate) fn recording_stream_url(port: u16, recording_media_id: Uuid) -> String {
    format!("http://127.0.0.1:{port}/livetv/liverecordings/{recording_media_id}/stream")
}

/// The single playable `Stream`-kind child of a synced `Recording` row,
/// pointing at `recording_stream_url`. A recording still being written gets
/// a different id from the finished file, so a probe of the growing playlist
/// is not reused for the file.
pub(crate) fn recording_stream_to_media(
    rec: &super::dispatcharr_dvr::DispatcharrRecording,
    recording_media_id: Uuid,
    stream_url: &str,
    now: chrono::NaiveDateTime,
) -> db::Media {
    let key: &[u8] = if rec.status()
        == super::dispatcharr_dvr::DispatcharrRecordingStatus::Recording
    {
        b"stream:recording"
    } else {
        b"stream"
    };
    db::Media {
        id: Uuid::new_v5(&recording_media_id, key),
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

/// How many channels the enabled `channels` catalog imports, resolved as
/// `RefreshIptvTask` resolves it: the catalog's own limit, else `global_max`.
/// `None` when that catalog is disabled.
pub(crate) fn channel_limit_of(
    catalogs: &[super::ResolvedCatalog],
    global_max: usize,
) -> Option<usize> {
    catalogs
        .iter()
        .find(|c| c.enabled && c.provider_catalog_id == "channels")
        .map(|c| {
            c.max_items
                .map_or(global_max, |n| n.max(0) as usize)
        })
}

/// The global catalog limit `RefreshIptvTask` falls back to.
pub(crate) async fn global_catalog_max(ctx: &AppContext) -> usize {
    db::Settings::get_config_or_default(&ctx.db)
        .await
        .catalog_max_items
        .unwrap_or(250) as usize
}

/// `channel_limit_of` for one addon instance, looked up from its catalogs.
pub(crate) async fn channel_limit(ctx: &AppContext, addon_id: Uuid) -> Option<usize> {
    let global_max = global_catalog_max(ctx).await;
    ctx.addons
        .catalogs_for_kinds(ctx, &[db::MediaKind::TvChannel])
        .await
        .into_iter()
        .find(|(runtime, _)| {
            runtime
                .row
                .id
                == addon_id
        })
        .and_then(|(_, catalogs)| channel_limit_of(&catalogs, global_max))
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
        // Channel versions are the synced `Stream` children; echo the row, as
        // `IptvAddon` does.
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

    fn ts(s: &str) -> chrono::DateTime<chrono::Utc> {
        s.parse()
            .unwrap()
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

    #[test]
    fn resolution_and_fps_labels_come_from_the_probe() {
        for (height, want) in [
            (2160, "4K"),
            (2000, "4K"),
            (1999, "FHD"),
            (1000, "FHD"),
            (999, "HD"),
            (700, "HD"),
            (699, "SD"),
            (540, "SD"),
        ] {
            assert_eq!(
                resolution_label(&stats(height, 25.0)),
                Some(want),
                "{height}p"
            );
        }
        assert_eq!(
            resolution_label(
                &json!({ "height": 1080, "resolution": { "height": 2160 } })
            ),
            Some("4K"),
            "a nested resolution height wins"
        );
        // Only a numeric height counts, not the "WxH" string.
        for stats in [
            json!({}),
            json!({ "height": null }),
            json!({ "resolution": "1920x1080" }),
        ] {
            assert_eq!(resolution_label(&stats), None, "{stats}");
        }

        for (fps, want) in [(50.0, "50fps"), (23.976, "24fps"), (59.94, "60fps")] {
            assert_eq!(fps_label(&stats(1080, fps)).as_deref(), Some(want));
        }
        for fps in [json!(null), json!(0), json!(-25.0), json!("50")] {
            assert_eq!(fps_label(&json!({ "source_fps": fps })), None, "{fps}");
        }
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
            ('x', 'x'),
            ('Z', 'Z'),
            ('7', '7'),
        ] {
            assert_eq!(fold_superscript(from), to, "{from}");
        }
    }

    #[test]
    fn clean_stream_name_strips_prefixes_and_quality_tags() {
        for (raw, want) in [
            ("ES: LA 2 ᵁᴴᴰ", "LA 2"),
            ("ES: LA 2 SD", "LA 2"),
            ("GO: LA 2 ᴿᴬᵂ", "LA 2"),
            ("AV: LA2 ᴿᴬᵂ", "LA2"),
            ("|UK| Sky Sports Main Event 4K", "Sky Sports Main Event"),
            ("UK | BBC One FHD 50FPS", "BBC One"),
            ("Sky Cinema (HD)", "Sky Cinema"),
            ("Eurosport 1 1080p50", "Eurosport 1"),
            ("Discovery ⁶⁰ᶠᵖˢ", "Discovery"),
            ("⁴ᴷ Movies", "Movies"),
            ("BBC WORLD NEWS ʰᵉᵛᶜ", "BBC WORLD NEWS"),
            ("DAZN 1 H.265 HDR10", "DAZN 1"),
            ("  ES:   LA 2   ᴴᴰ  ", "LA 2"),
            // Ordinary names are left alone.
            ("Channel 4", "Channel 4"),
            ("Sky Sports F1", "Sky Sports F1"),
            ("CBBC", "CBBC"),
            ("Sky: News", "Sky: News"),
            ("Movistar Plus+ 1", "Movistar Plus+ 1"),
            // Nothing would be left, so the raw name stays.
            ("HD", "HD"),
            (" ES: ᴴᴰ ", "ES: ᴴᴰ"),
        ] {
            assert_eq!(clean_stream_name(raw), want, "{raw}");
        }
    }

    #[test]
    fn stream_title_reads_resolution_then_fps_then_name() {
        let titled = |name: &str, stats: Option<serde_json::Value>| {
            stream_title(&stream(
                json!({ "id": 1, "name": name, "stream_stats": stats }),
            ))
        };
        assert_eq!(
            titled("ES: LA 2 ᵁᴴᴰ", Some(stats(1080, 50.0))),
            "FHD · 50fps · LA 2"
        );
        assert_eq!(
            titled("Sky Sports 4K", Some(stats(2160, 50.0))),
            "4K · 50fps · Sky Sports"
        );
        // Providers mislabel: this "SD" stream probes as 1080p.
        assert_eq!(
            titled("ES: LA 2 SD", Some(stats(1080, 30.0))),
            "FHD · 30fps · LA 2"
        );
        // What wasn't probed is left out.
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

    // -- ids and limits -----------------------------------------------

    #[test]
    fn a_programme_id_round_trips_as_the_json_type_it_came_as() {
        assert_eq!(program_key(&json!(276710)).as_deref(), Some("276710"));
        assert_eq!(program_key_value("276710"), json!(276710));
        assert_eq!(
            program_key(&json!("dummy-12-3")).as_deref(),
            Some("dummy-12-3")
        );
        assert_eq!(program_key_value("dummy-12-3"), json!("dummy-12-3"));
        assert_eq!(program_key(&json!(null)), None);
        assert_eq!(program_key(&json!("")), None);
    }

    #[test]
    fn the_channel_limit_follows_the_catalog_then_the_global_setting() {
        let catalog = |enabled, max_items| super::super::ResolvedCatalog {
            provider_catalog_id: "channels".into(),
            catalog_id: "addon:x:channels".into(),
            collection_id: Uuid::nil(),
            name: "Dispatcharr Channels".into(),
            media_kind: Some(db::MediaKind::TvChannel),
            collection_media_kind: None,
            enabled,
            max_items,
            tags: vec![],
        };
        assert_eq!(
            channel_limit_of(&[catalog(true, Some(100))], 250),
            Some(100)
        );
        assert_eq!(channel_limit_of(&[catalog(true, None)], 250), Some(250));
        assert_eq!(channel_limit_of(&[catalog(false, Some(100))], 250), None);
        assert_eq!(channel_limit_of(&[], 250), None);
    }

    #[test]
    fn epg_window_reads_the_addon_options() {
        assert_eq!(EpgWindow::from_cfg(&json!({})), EpgWindow::DEFAULT);
        assert_eq!(
            EpgWindow::from_cfg(&json!({
                "epg_lookback_hours": 6, "epg_lookahead_hours": "168",
            })),
            EpgWindow {
                lookback_hours: 6,
                lookahead_hours: 168,
            }
        );
        assert_eq!(
            EpgWindow::from_cfg(&json!({
                "epg_lookback_hours": -1, "epg_lookahead_hours": 0,
            })),
            EpgWindow {
                lookback_hours: 0,
                lookahead_hours: 1,
            }
        );
        assert_eq!(
            EpgWindow::from_cfg(&json!({ "epg_lookback_hours": "soon" })),
            EpgWindow::DEFAULT
        );
    }

    // -- rows -----------------------------------------------------------

    #[test]
    fn channel_to_media_maps_fields() {
        let ch = channel(json!({
            "id": 42, "uuid": "u-42", "name": "BBC One", "channel_number": 7.5,
            "tvg_id": "BBC1.uk",
        }));
        let m = channel_to_media(&ch, ADDON, "src");
        assert_eq!(m.id, Uuid::new_v5(&ADDON, b"channel:42"));
        assert_eq!(m.title, "BBC One");
        assert_eq!(m.kind, db::MediaKind::TvChannel);
        assert_eq!(
            m.channel_number,
            Some(7),
            "fractional numbers are truncated"
        );
        assert_eq!(
            m.external_ids
                .iptv_source_id
                .as_deref(),
            Some("src")
        );
        // The guide is matched through `epg_data_id`, not this copy.
        assert_eq!(m.tvg_id, None);
        assert!(m.enabled);
        assert_ne!(m.id, channel_to_media(&ch, Uuid::from_u128(1), "s").id);
        let bare = channel(json!({ "id": 2, "uuid": "u", "name": "B" }));
        assert_eq!(channel_to_media(&bare, ADDON, "s").channel_number, None);
    }

    #[test]
    fn stream_to_media_points_at_the_proxy_with_the_api_key() {
        let s = stream(json!({
            "id": 7, "name": "ES: LA 2 SD", "stream_hash": "h",
            "stream_stats": { "height": 540, "resolution": "960x540", "source_fps": 25.0 },
        }));
        let parent = Uuid::from_u128(1);
        let m = stream_to_media(
            &s,
            parent,
            3,
            "chan-uuid",
            "http://d:9191",
            "secret",
            now(),
        );
        assert_eq!(m.id, Uuid::new_v5(&parent, b"stream:7"));
        assert_eq!(m.title, "SD · 25fps · LA 2");
        assert_eq!(
            (&m.kind, m.parent_id, m.idx),
            (&db::MediaKind::Stream, Some(parent), Some(3))
        );
        let (url, headers) = http_parts(&m);
        // A trailing slash falls through to Dispatcharr's SPA catch-all.
        assert_eq!(url, "http://d:9191/proxy/ts/stream/chan-uuid");
        assert_eq!(
            headers
                .iter()
                .collect::<Vec<_>>(),
            [(&"X-API-Key".to_string(), &"secret".to_string())]
        );
    }

    #[test]
    fn recording_to_media_maps_fields() {
        let m = recording_to_media(&sample_recording(), ADDON, "src");
        assert_eq!(m.id, Uuid::new_v5(&ADDON, b"recording:5"));
        assert_eq!(m.kind, db::MediaKind::Recording);
        assert_eq!(m.title, "Match of the Day");
        assert_eq!(
            m.description
                .as_deref(),
            Some("Highlights.")
        );
        assert_eq!(m.runtime, Some(90 * 60));
        assert_eq!(
            (m.live_start, m.live_end),
            (
                Some(ts("2026-09-15T20:00:00Z").naive_utc()),
                Some(ts("2026-09-15T21:30:00Z").naive_utc())
            )
        );
        assert_eq!(
            m.external_ids
                .dispatcharr_recording_id,
            Some(5)
        );
        // The FK only holds if both sides derive the channel id the same way.
        let ch = channel(json!({ "id": 42, "uuid": "u", "name": "BBC One" }));
        assert_eq!(m.parent_id, Some(channel_to_media(&ch, ADDON, "src").id));

        // Unnamed, and ending before it starts.
        let odd = recording(json!({
            "id": 1, "channel": 1,
            "start_time": "2026-09-15T21:00:00Z",
            "end_time": "2026-09-15T20:00:00Z",
        }));
        let m = recording_to_media(&odd, ADDON, "s");
        assert_eq!(
            (
                m.title
                    .as_str(),
                m.description
            ),
            ("Recording", None)
        );
        assert_eq!(m.runtime, Some(0));
    }

    #[test]
    fn recording_stream_points_at_remuxs_own_endpoint() {
        let rec = sample_recording();
        let parent = recording_to_media(&rec, ADDON, "s");
        let url = recording_stream_url(3000, parent.id);
        let m = recording_stream_to_media(&rec, parent.id, &url, now());
        assert_eq!(m.id, Uuid::new_v5(&parent.id, b"stream"));
        assert_eq!(
            (&m.kind, m.parent_id, m.idx),
            (&db::MediaKind::Stream, Some(parent.id), Some(0))
        );
        let mut running = rec.clone();
        running.custom_properties["status"] = json!("recording");
        assert_ne!(
            recording_stream_to_media(&running, parent.id, &url, now()).id,
            m.id,
            "the growing playlist and the finished file are different rows"
        );
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

    fn versions(streams: &[DispatcharrStream]) -> Vec<db::Media> {
        let ch = channel(json!({ "id": 42, "uuid": "chan-uuid", "name": "BBC" }));
        channel_versions(&ch, streams, Uuid::from_u128(1), "http://d", "k", now())
    }

    fn hashed(n: usize) -> Vec<DispatcharrStream> {
        (0..n)
            .map(|i| {
                stream(json!({
                    "id": 100 + i, "name": format!("s{i}"),
                    "stream_hash": format!("hash{i}"),
                }))
            })
            .collect()
    }

    fn urls(rows: &[db::Media]) -> Vec<&str> {
        rows.iter()
            .map(|m| http_parts(m).0)
            .collect()
    }

    #[test]
    fn every_stream_gets_a_pinned_version_after_the_default() {
        let rows = versions(&hashed(3));
        assert_eq!(
            rows.iter()
                .map(|m| m.idx)
                .collect::<Vec<_>>(),
            [Some(0), Some(1), Some(2), Some(3)]
        );
        assert_eq!(
            urls(&rows),
            [
                // default: Dispatcharr picks
                "http://d/proxy/ts/stream/chan-uuid",
                "http://d/proxy/ts/stream/hash0",
                "http://d/proxy/ts/stream/hash1",
                "http://d/proxy/ts/stream/hash2",
            ]
        );
        // Stable ids: the default keeps the first stream's established id.
        let parent = Uuid::from_u128(1);
        let ids: Vec<_> = rows
            .iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(
            ids,
            [
                Uuid::new_v5(&parent, b"stream:100"),
                Uuid::new_v5(&parent, b"pinned:100"),
                Uuid::new_v5(&parent, b"stream:101"),
                Uuid::new_v5(&parent, b"stream:102"),
            ]
        );
    }

    #[test]
    fn only_a_choice_between_hashed_streams_gets_pinned_versions() {
        assert_eq!(
            urls(&versions(&hashed(1))),
            ["http://d/proxy/ts/stream/chan-uuid"]
        );
        assert!(versions(&[]).is_empty());
        let unhashed = [
            stream(json!({ "id": 7, "name": "a" })),
            stream(json!({ "id": 8, "name": "b", "stream_hash": "h8" })),
        ];
        assert_eq!(
            urls(&versions(&unhashed)),
            [
                "http://d/proxy/ts/stream/chan-uuid",
                "http://d/proxy/ts/stream/h8",
            ]
        );
    }

    // -- preset ---------------------------------------------------------

    #[test]
    fn from_cfg_requires_base_url_and_api_key() {
        let cfg = crate::Config::default();
        let build =
            |v: serde_json::Value| DispatcharrPreset.from_cfg(Uuid::nil(), &v, &cfg);

        assert!(build(json!({ "base_url": "http://d", "api_key": "k" })).is_ok());
        for (bad, missing) in [
            (json!({}), "base_url"),
            (json!({ "api_key": "k" }), "base_url"),
            (json!({ "base_url": "http://d" }), "api_key"),
            (json!({ "base_url": "", "api_key": "k" }), "base_url"),
            (json!({ "base_url": "http://d", "api_key": "" }), "api_key"),
        ] {
            let err = build(bad.clone())
                .err()
                .unwrap_or_else(|| panic!("should reject {bad}"))
                .to_string();
            assert!(err.contains(missing), "{bad}: {err}");
        }
    }

    // -- HTTP client --------------------------------------------------

    #[tokio::test]
    async fn fetchers_send_the_api_key_and_parse_dispatcharrs_responses() {
        let server = httpmock::MockServer::start();
        let get = |path: &str, body: serde_json::Value| {
            server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path(path)
                    .header("X-API-Key", "k");
                then.status(200)
                    .json_body(body);
            })
        };
        get(
            "/api/channels/channels/",
            json!([
                { "id": 1, "uuid": "a", "name": "One", "epg_data_id": 10 },
                { "id": 2, "uuid": "b", "name": "Two" },
            ]),
        );
        get(
            "/api/channels/channels/749/streams/",
            json!([
                { "id": 30, "name": "first", "stream_hash": "h30" },
                { "id": 10, "name": "second", "stream_hash": null },
            ]),
        );
        get(
            "/api/epg/epgdata/",
            json!([
                { "id": 10, "tvg_id": "BBC1.uk", "epg_source": 3 },
                { "id": 11, "tvg_id": null },
            ]),
        );
        let client = reqwest::Client::new();
        let base = server.base_url();

        let channels = fetch_channels(&client, &base, "k")
            .await
            .unwrap();
        assert_eq!(
            channels
                .iter()
                .map(|c| (c.id, c.epg_data_id))
                .collect::<Vec<_>>(),
            [(1, Some(10)), (2, None)]
        );
        // Dispatcharr's order is the version order.
        let streams = fetch_channel_streams(&client, &base, "k", 749)
            .await
            .unwrap();
        assert_eq!(
            streams
                .iter()
                .map(|s| (
                    s.id,
                    s.stream_hash
                        .as_deref()
                ))
                .collect::<Vec<_>>(),
            [(30, Some("h30")), (10, None)]
        );
        let epg = fetch_epg_data(&client, &base, "k")
            .await
            .unwrap();
        assert_eq!(
            epg.iter()
                .map(|d| (
                    d.tvg_id
                        .as_deref(),
                    d.epg_source
                ))
                .collect::<Vec<_>>(),
            [(Some("BBC1.uk"), Some(3)), (None, None)]
        );
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
        let now = chrono::Utc::now();
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
            fetch_epg_grid(&client, &base, "bad", now, now)
                .await
                .is_err()
        );
        assert!(
            fetch_source_programs(&client, &base, "bad", "t", 1, now, now)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn fetch_epg_grid_asks_for_the_window_and_unwraps_the_envelope() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/epg/grid/")
                .query_param("start", "2026-09-15T18:00:00+00:00")
                .query_param("end", "2026-09-18T20:00:00+00:00")
                .header("X-API-Key", "k");
            then.status(200)
                .json_body(json!({ "data": [
                    {
                        "start_time": "2026-09-15T20:00:00Z",
                        "end_time": "2026-09-15T21:00:00Z",
                        "title": "News", "sub_title": "Late", "tvg_id": "BBC1.uk",
                    },
                    {
                        "start_time": "2026-09-15T20:00:00Z",
                        "end_time": "2026-09-15T21:00:00Z",
                        "title": null, "tvg_id": null,
                    },
                ]}));
        });
        let got = fetch_epg_grid(
            &reqwest::Client::new(),
            &server.base_url(),
            "k",
            ts("2026-09-15T18:00:00Z"),
            ts("2026-09-18T20:00:00Z"),
        )
        .await
        .unwrap();
        // Null fields are read, not a failure of the whole response.
        assert_eq!(
            got.iter()
                .map(|p| (
                    p.title
                        .as_deref(),
                    p.sub_title
                        .as_deref(),
                    p.tvg_id
                        .as_deref()
                ))
                .collect::<Vec<_>>(),
            [
                (Some("News"), Some("Late"), Some("BBC1.uk")),
                (None, None, None)
            ]
        );
    }

    #[tokio::test]
    async fn fetch_source_programs_follows_every_page() {
        let server = httpmock::MockServer::start();
        let prog = |title: &str| {
            json!({
                "start_time": "2026-09-15T20:00:00Z",
                "end_time": "2026-09-15T21:00:00Z",
                "title": title, "tvg_id": "BBC1.uk", "epg_source": "Sky",
            })
        };
        let first = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/epg/programs/search/")
                .query_param("tvg_id", "BBC1.uk")
                .query_param("epg_source", "3")
                .query_param("page", "1");
            then.status(200)
                .json_body(json!({ "count": 2, "next": "http://d/?page=2", "results": [prog("A")] }));
        });
        let second = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/epg/programs/search/")
                .query_param("page", "2");
            then.status(200)
                .json_body(json!({ "count": 2, "next": null, "results": [prog("B")] }));
        });
        let now = chrono::Utc::now();
        let got = fetch_source_programs(
            &reqwest::Client::new(),
            &server.base_url(),
            "k",
            "BBC1.uk",
            3,
            now,
            now,
        )
        .await
        .unwrap();
        first.assert();
        second.assert();
        let titles: Vec<_> = got
            .iter()
            .map(|p| {
                p.title
                    .as_deref()
            })
            .collect();
        assert_eq!(titles, [Some("A"), Some("B")]);
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
