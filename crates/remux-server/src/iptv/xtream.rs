use std::{collections::HashMap, fmt};

use anyhow::Result;
use chrono::NaiveDateTime;
use futures::TryStreamExt;
use serde::{
    Deserialize,
    de::{Deserializer as _, SeqAccess, Visitor},
};
use tokio_util::io::{StreamReader, SyncIoBridge};
use uuid::Uuid;

use super::M3uChannel;
use crate::{db, db::ProgramKind};

/// A single live stream entry from the Xtream Codes player API.
#[derive(Debug, Deserialize)]
pub struct XtreamStream {
    pub num: Option<i64>,
    pub name: Option<String>,
    pub stream_id: Option<serde_json::Value>, // some providers send int, some string
    pub stream_icon: Option<String>,
    pub epg_channel_id: Option<String>,
    pub category_id: Option<serde_json::Value>, // int or string depending on provider
    pub category_name: Option<String>,
}

/// A category entry from the Xtream Codes `get_live_categories` endpoint.
#[derive(Debug, Deserialize)]
pub struct XtreamCategory {
    pub category_id: Option<serde_json::Value>,
    pub category_name: Option<String>,
}

/// Fetch live categories and return a map of `category_id → ProgramKind`.
/// Returns an empty map on failure (non-fatal).
pub async fn fetch_xtream_categories(
    client: &reqwest::Client,
    server: &str,
    username: &str,
    password: &str,
) -> HashMap<String, ProgramKind> {
    let base = server.trim_end_matches('/');
    let url = format!(
        "{}/player_api.php?username={}&password={}&action=get_live_categories",
        base, username, password
    );
    let Ok(resp) = client
        .get(&url)
        .send()
        .await
    else {
        return HashMap::new();
    };
    let Ok(cats) = resp
        .json::<Vec<XtreamCategory>>()
        .await
    else {
        return HashMap::new();
    };
    cats.into_iter()
        .filter_map(|c| {
            let id = json_value_to_string(
                c.category_id
                    .as_ref()?,
            )?;
            let kind = super::parse_program_kind(
                c.category_name
                    .as_deref()?,
            )?;
            Some((id, kind))
        })
        .collect()
}

/// Fetch live channels from the Xtream Codes player API and return them as
/// `M3uChannel` values so the rest of the import pipeline is reused unchanged.
///
/// Uses a streaming JSON visitor so only one `XtreamStream` exists in memory at
/// a time — the full deserialized array is never materialised.
pub async fn fetch_xtream_channels(
    client: &reqwest::Client,
    server: &str,
    username: &str,
    password: &str,
    category_kinds: &HashMap<String, ProgramKind>,
) -> Result<Vec<M3uChannel>> {
    let base = server
        .trim_end_matches('/')
        .to_owned();
    let url = format!(
        "{}/player_api.php?username={}&password={}&action=get_live_streams",
        base, username, password
    );
    let username = username.to_owned();
    let password = password.to_owned();
    let category_kinds = category_kinds.clone();

    let resp = client
        .get(&url)
        .send()
        .await?;
    let byte_stream = resp
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let async_reader = StreamReader::new(byte_stream);
    let handle = tokio::runtime::Handle::current();

    tokio::task::spawn_blocking(move || -> Result<Vec<M3uChannel>> {
        let sync_reader = SyncIoBridge::new_with_handle(async_reader, handle);
        let buf_reader = std::io::BufReader::with_capacity(256 * 1024, sync_reader);
        let mut de = serde_json::Deserializer::from_reader(buf_reader);

        // Visitor: parses the outer JSON array element by element, converts each
        // XtreamStream to M3uChannel immediately, then drops the stream struct.
        struct StreamCollector {
            base: String,
            username: String,
            password: String,
            category_kinds: HashMap<String, ProgramKind>,
        }

        impl<'de> Visitor<'de> for StreamCollector {
            type Value = Vec<M3uChannel>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "an array of Xtream stream objects")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Vec<M3uChannel>, A::Error> {
                let mut channels = Vec::new();
                while let Some(stream) = seq.next_element::<XtreamStream>()? {
                    if let Some(ch) = stream_to_channel(
                        stream,
                        &self.base,
                        &self.username,
                        &self.password,
                        &self.category_kinds,
                    ) {
                        channels.push(ch);
                    }
                }
                Ok(channels)
            }
        }

        de.deserialize_seq(StreamCollector {
            base,
            username,
            password,
            category_kinds,
        })
        .map_err(anyhow::Error::from)
    })
    .await?
}

fn stream_to_channel(
    s: XtreamStream,
    base: &str,
    username: &str,
    password: &str,
    category_kinds: &HashMap<String, ProgramKind>,
) -> Option<M3uChannel> {
    let name = s
        .name
        .filter(|n| !n.is_empty())?;
    let stream_id = stream_id_to_string(&s.stream_id?)?;
    let stream_url = format!("{}/{}/{}/{}", base, username, password, stream_id);

    let program_kind = s
        .category_id
        .as_ref()
        .and_then(json_value_to_string)
        .as_deref()
        .and_then(|id| category_kinds.get(id))
        .cloned()
        .or_else(|| {
            s.category_name
                .as_deref()
                .and_then(super::parse_program_kind)
        });

    let group = s
        .category_name
        .filter(|s| !s.is_empty());

    Some(M3uChannel {
        tvg_id: s
            .epg_channel_id
            .filter(|s| !s.is_empty()),
        name,
        logo: s
            .stream_icon
            .filter(|s| !s.is_empty()),
        group,
        channel_number: s.num,
        url: stream_url,
        program_kind,
        language: None,
        catchup: None,
        catchup_days: None,
        catchup_source: None,
    })
}

fn stream_id_to_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn json_value_to_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// VOD (movies)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct XtreamVodStream {
    pub name: Option<String>,
    pub stream_id: Option<serde_json::Value>,
    pub stream_icon: Option<String>,
    pub category_id: Option<serde_json::Value>,
    pub category_name: Option<String>,
    pub plot: Option<String>,
    pub genre: Option<String>,
    pub releasedate: Option<String>,
    pub rating: Option<serde_json::Value>,
    pub container_extension: Option<String>,
}

/// Fetch VOD categories and return `category_id → ProgramKind` (best-effort).
pub async fn fetch_vod_categories(
    client: &reqwest::Client,
    server: &str,
    username: &str,
    password: &str,
) -> HashMap<String, ProgramKind> {
    let base = server.trim_end_matches('/');
    let url = format!(
        "{}/player_api.php?username={}&password={}&action=get_vod_categories",
        base, username, password
    );
    let Ok(resp) = client
        .get(&url)
        .send()
        .await
    else {
        return HashMap::new();
    };
    let Ok(cats) = resp
        .json::<Vec<XtreamCategory>>()
        .await
    else {
        return HashMap::new();
    };
    cats.into_iter()
        .filter_map(|c| {
            let id = json_value_to_string(
                c.category_id
                    .as_ref()?,
            )?;
            let kind = super::parse_program_kind(
                c.category_name
                    .as_deref()?,
            )?;
            Some((id, kind))
        })
        .collect()
}

/// Fetch VOD streams and convert them to `db::Media` items (kind=Movie).
pub async fn fetch_vod_streams(
    client: &reqwest::Client,
    server: &str,
    username: &str,
    password: &str,
    addon_id: Uuid,
    source_id: &str,
) -> Result<Vec<db::Media>> {
    let base = server
        .trim_end_matches('/')
        .to_owned();
    let url = format!(
        "{}/player_api.php?username={}&password={}&action=get_vod_streams",
        base, username, password
    );
    let username = username.to_owned();
    let password = password.to_owned();
    let source_id = source_id.to_owned();
    let category_kinds =
        fetch_vod_categories(client, server, &username, &password).await;

    let resp = client
        .get(&url)
        .send()
        .await?;
    let byte_stream = resp
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let async_reader = StreamReader::new(byte_stream);
    let handle = tokio::runtime::Handle::current();

    tokio::task::spawn_blocking(move || -> Result<Vec<db::Media>> {
        let sync_reader = SyncIoBridge::new_with_handle(async_reader, handle);
        let buf_reader = std::io::BufReader::with_capacity(256 * 1024, sync_reader);
        let mut de = serde_json::Deserializer::from_reader(buf_reader);

        struct VodCollector {
            base: String,
            username: String,
            password: String,
            addon_id: Uuid,
            source_id: String,
            category_kinds: HashMap<String, ProgramKind>,
        }

        impl<'de> Visitor<'de> for VodCollector {
            type Value = Vec<db::Media>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "an array of Xtream VOD objects")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Vec<db::Media>, A::Error> {
                let mut items = Vec::new();
                while let Some(vod) = seq.next_element::<XtreamVodStream>()? {
                    if let Some(m) = vod_to_media(
                        vod,
                        &self.base,
                        &self.username,
                        &self.password,
                        self.addon_id,
                        &self.source_id,
                        &self.category_kinds,
                    ) {
                        items.push(m);
                    }
                }
                Ok(items)
            }
        }

        de.deserialize_seq(VodCollector {
            base,
            username,
            password,
            addon_id,
            source_id,
            category_kinds,
        })
        .map_err(anyhow::Error::from)
    })
    .await?
}

fn vod_to_media(
    v: XtreamVodStream,
    base: &str,
    username: &str,
    password: &str,
    addon_id: Uuid,
    source_id: &str,
    category_kinds: &HashMap<String, ProgramKind>,
) -> Option<db::Media> {
    let name = v
        .name
        .filter(|n| !n.is_empty())?;
    let stream_id = stream_id_to_string(&v.stream_id?)?;
    let ext = v
        .container_extension
        .as_deref()
        .unwrap_or("mp4");
    let stream_url =
        format!("{}/{}/{}/{}.{}", base, username, password, stream_id, ext);

    let program_kind = v
        .category_id
        .as_ref()
        .and_then(json_value_to_string)
        .as_deref()
        .and_then(|id| category_kinds.get(id))
        .cloned()
        .or_else(|| {
            v.category_name
                .as_deref()
                .and_then(super::parse_program_kind)
        });

    let released_at = v
        .releasedate
        .as_deref()
        .and_then(parse_year_or_date);
    let rating_audience = v
        .rating
        .as_ref()
        .and_then(parse_rating);

    let media_id = Uuid::new_v5(&addon_id, format!("vod:{}", stream_id).as_bytes());
    let mut media = db::Media {
        id: media_id,
        title: name,
        kind: db::MediaKind::Movie,
        description: v
            .plot
            .filter(|s| !s.is_empty()),
        released_at,
        rating_audience,
        external_ids: db::ExternalIds {
            iptv_source_id: Some(source_id.to_owned()),
            iptv_group: v
                .category_name
                .filter(|s| !s.is_empty()),
            ..Default::default()
        },
        stream_info: Some(crate::stream::StreamInfo {
            descriptor: crate::stream::StreamDescriptor::http(stream_url),
            ..Default::default()
        }),
        program_kind,
        ..Default::default()
    };
    if let Some(url) = v
        .stream_icon
        .filter(|s| !s.is_empty())
    {
        media.set_image(db::ImageKind::Primary, url);
    }
    Some(media)
}

// ---------------------------------------------------------------------------
// Series
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct XtreamSeriesItem {
    pub name: Option<String>,
    pub series_id: Option<serde_json::Value>,
    pub cover: Option<String>,
    pub category_id: Option<serde_json::Value>,
    pub category_name: Option<String>,
    pub plot: Option<String>,
    pub genre: Option<String>,
    pub releasedate: Option<String>,
    pub rating: Option<serde_json::Value>,
}

/// Fetch series categories and return `category_id → ProgramKind`.
pub async fn fetch_series_categories(
    client: &reqwest::Client,
    server: &str,
    username: &str,
    password: &str,
) -> HashMap<String, ProgramKind> {
    let base = server.trim_end_matches('/');
    let url = format!(
        "{}/player_api.php?username={}&password={}&action=get_series_categories",
        base, username, password
    );
    let Ok(resp) = client
        .get(&url)
        .send()
        .await
    else {
        return HashMap::new();
    };
    let Ok(cats) = resp
        .json::<Vec<XtreamCategory>>()
        .await
    else {
        return HashMap::new();
    };
    cats.into_iter()
        .filter_map(|c| {
            let id = json_value_to_string(
                c.category_id
                    .as_ref()?,
            )?;
            let kind = super::parse_program_kind(
                c.category_name
                    .as_deref()?,
            )?;
            Some((id, kind))
        })
        .collect()
}

/// Fetch the series list and convert to `db::Media` items (kind=Series).
/// Episodes are not fetched here — that requires a per-series API call.
pub async fn fetch_series_list(
    client: &reqwest::Client,
    server: &str,
    username: &str,
    password: &str,
    addon_id: Uuid,
    source_id: &str,
) -> Result<Vec<db::Media>> {
    let base = server
        .trim_end_matches('/')
        .to_owned();
    let url = format!(
        "{}/player_api.php?username={}&password={}&action=get_series",
        base, username, password
    );
    let source_id = source_id.to_owned();
    let category_kinds =
        fetch_series_categories(client, server, username, password).await;

    let resp = client
        .get(&url)
        .send()
        .await?;
    let byte_stream = resp
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let async_reader = StreamReader::new(byte_stream);
    let handle = tokio::runtime::Handle::current();

    tokio::task::spawn_blocking(move || -> Result<Vec<db::Media>> {
        let sync_reader = SyncIoBridge::new_with_handle(async_reader, handle);
        let buf_reader = std::io::BufReader::with_capacity(256 * 1024, sync_reader);
        let mut de = serde_json::Deserializer::from_reader(buf_reader);

        struct SeriesCollector {
            addon_id: Uuid,
            source_id: String,
            category_kinds: HashMap<String, ProgramKind>,
        }

        impl<'de> Visitor<'de> for SeriesCollector {
            type Value = Vec<db::Media>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "an array of Xtream series objects")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Vec<db::Media>, A::Error> {
                let mut items = Vec::new();
                while let Some(s) = seq.next_element::<XtreamSeriesItem>()? {
                    if let Some(m) = series_to_media(
                        s,
                        self.addon_id,
                        &self.source_id,
                        &self.category_kinds,
                    ) {
                        items.push(m);
                    }
                }
                Ok(items)
            }
        }

        de.deserialize_seq(SeriesCollector {
            addon_id,
            source_id,
            category_kinds,
        })
        .map_err(anyhow::Error::from)
    })
    .await?
}

fn series_to_media(
    s: XtreamSeriesItem,
    addon_id: Uuid,
    source_id: &str,
    category_kinds: &HashMap<String, ProgramKind>,
) -> Option<db::Media> {
    let name = s
        .name
        .filter(|n| !n.is_empty())?;
    let series_id = stream_id_to_string(&s.series_id?)?;

    let program_kind = s
        .category_id
        .as_ref()
        .and_then(json_value_to_string)
        .as_deref()
        .and_then(|id| category_kinds.get(id))
        .cloned()
        .or_else(|| {
            s.category_name
                .as_deref()
                .and_then(super::parse_program_kind)
        });

    let released_at = s
        .releasedate
        .as_deref()
        .and_then(parse_year_or_date);
    let rating_audience = s
        .rating
        .as_ref()
        .and_then(parse_rating);

    let media_id = Uuid::new_v5(&addon_id, format!("series:{}", series_id).as_bytes());
    let mut media = db::Media {
        id: media_id,
        title: name,
        kind: db::MediaKind::Series,
        description: s
            .plot
            .filter(|s| !s.is_empty()),
        released_at,
        rating_audience,
        external_ids: db::ExternalIds {
            iptv_source_id: Some(source_id.to_owned()),
            iptv_group: s
                .category_name
                .filter(|s| !s.is_empty()),
            ..Default::default()
        },
        program_kind,
        ..Default::default()
    };
    if let Some(url) = s
        .cover
        .filter(|s| !s.is_empty())
    {
        media.set_image(db::ImageKind::Primary, url);
    }
    Some(media)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn parse_year_or_date(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Try full date first, then year-only
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .or_else(|_| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
        .map(|d| {
            d.and_hms_opt(0, 0, 0)
                .unwrap()
        })
        .or_else(|_| {
            s.get(..4)
                .and_then(|y| {
                    y.parse::<i32>()
                        .ok()
                })
                .and_then(|y| chrono::NaiveDate::from_ymd_opt(y, 1, 1))
                .map(|d| {
                    d.and_hms_opt(0, 0, 0)
                        .unwrap()
                })
                .ok_or(())
        })
        .ok()
}

fn parse_rating(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s
            .split('/')
            .next()?
            .trim()
            .parse()
            .ok(),
        _ => None,
    }
}
