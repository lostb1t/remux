use anyhow::{Result, bail};
use async_trait::async_trait;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::sync::Arc;
use uuid::Uuid;

use super::{
    AddonKind, AddonMetadata, AddonOption, AddonOptionType, AddonPreset,
    AddonPresetRegistration, MediaKind, ResourceType,
};
use crate::{AppContext, api, db};

const MAX_RESULTS: usize = 10;
const MAX_CANDIDATES: usize = 100;

inventory::submit! {
    AddonPresetRegistration(|| Box::new(TorznabPreset))
}

pub struct TorznabPreset;

impl AddonPreset for TorznabPreset {
    fn id(&self) -> &'static str {
        "torznab"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "torznab".to_string(),
            display_name: "Torznab".to_string(),
            description: "Stream resolution via a Torznab-compatible indexer (e.g. Jackett, Prowlarr, Bitmagnet). Supports music tracks, movies, and TV episodes via magnet links.".to_string(),
            icon: None,
            supported_resources: vec![ResourceType::Stream],
            supported_types: vec![MediaKind::Track, MediaKind::Movie, MediaKind::Episode],
            options: vec![
                AddonOption {
                    id: "url".to_string(),
                    name: "API URL".to_string(),
                    description: Some("Torznab API endpoint URL (e.g. http://localhost:9117/api/v2.0/indexers/all/results/torznab/api)".to_string()),
                    required: true,
                    default: None,
                    kind: AddonOptionType::Url,
                },
                AddonOption {
                    id: "name".to_string(),
                    name: "Display name".to_string(),
                    description: Some("Label shown in stream results to identify this indexer.".to_string()),
                    required: false,
                    default: Some(serde_json::Value::String("torznab".to_string())),
                    kind: AddonOptionType::String,
                },
            ],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<Arc<dyn AddonKind>> {
        let url = cfg["url"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("torznab: 'url' is required"))?
            .to_string();
        let name = cfg["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("torznab")
            .to_string();

        Ok(Arc::new(TorznabAddon {
            url,
            name,
            client: build_client(),
        }))
    }
}

pub struct TorznabAddon {
    url: String,
    name: String,
    client: reqwest::Client,
}

#[async_trait]
impl AddonKind for TorznabAddon {
    fn id(&self) -> &'static str {
        "torznab"
    }

    fn stream_supports(&self, media: &db::Media) -> bool {
        matches!(
            media.kind,
            db::MediaKind::Track | db::MediaKind::Movie | db::MediaKind::Episode
        )
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        match media.kind {
            db::MediaKind::Track => {
                self.resolve_track(media)
                    .await
            }
            db::MediaKind::Movie => {
                self.resolve_movie(media)
                    .await
            }
            db::MediaKind::Episode => {
                self.resolve_episode(media)
                    .await
            }
            _ => Ok(vec![]),
        }
    }
}

impl TorznabAddon {
    async fn resolve_track(
        &self,
        media: &db::Media,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        let artist = media
            .description
            .as_deref()
            .and_then(|d| d.strip_prefix("by "))
            .filter(|a| {
                !a.trim()
                    .is_empty()
            })
            .map(str::to_string);

        let search = MediaSearch {
            title: media
                .title
                .clone(),
            extra: artist.clone(),
            title_tokens: significant_tokens(&media.title),
            extra_tokens: artist
                .as_deref()
                .map(significant_tokens)
                .unwrap_or_default(),
        };

        let query = match artist.as_deref() {
            Some(a) => format!("{} {}", a, media.title),
            None => media
                .title
                .clone(),
        };

        tracing::debug!(query, title = %media.title, "torznab track stream lookup");

        let mut items = self
            .fetch_items(&query, "3000")
            .await?;
        items.retain(|item| {
            item.is_music()
                && item
                    .url()
                    .is_some()
        });
        if items
            .iter()
            .any(|item| item_matches_title(item, &search))
        {
            items.retain(|item| item_matches_title(item, &search));
        }
        items.sort_by(|a, b| {
            score_item(b, &search)
                .cmp(&score_item(a, &search))
                .then_with(|| {
                    b.seeders
                        .cmp(&a.seeders)
                })
                .then_with(|| {
                    b.peers
                        .cmp(&a.peers)
                })
                .then_with(|| {
                    a.size
                        .unwrap_or(i64::MAX)
                        .cmp(
                            &b.size
                                .unwrap_or(i64::MAX),
                        )
                })
        });

        Ok(items
            .into_iter()
            .take(MAX_RESULTS)
            .filter_map(|item| {
                let fmt = infer_audio_format(&item.title);
                let codec = fmt.map(|f| {
                    f.codec()
                        .to_string()
                });
                let descriptor = magnet_to_descriptor(
                    &item.url()?,
                    Some(
                        search
                            .title
                            .clone(),
                    ),
                )?;
                Some(crate::stream::StreamInfo {
                    descriptor,
                    name: Some(item.label(&self.name)),
                    probe_data: Some(api::MediaSourceInfo {
                        media_streams: vec![api::MediaStream {
                            index: 0,
                            type_: Some(api::MediaStreamType::Audio),
                            codec,
                            is_default: Some(true),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            })
            .collect())
    }

    async fn resolve_movie(
        &self,
        media: &db::Media,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        let year = media
            .released_at
            .map(|d| {
                d.format("%Y")
                    .to_string()
            });

        let query = match year.as_deref() {
            Some(y) => format!("{} {}", media.title, y),
            None => media
                .title
                .clone(),
        };

        tracing::debug!(query, title = %media.title, "torznab movie stream lookup");

        let search = MediaSearch {
            title: media
                .title
                .clone(),
            extra: year,
            title_tokens: significant_tokens(&media.title),
            extra_tokens: vec![],
        };

        let mut items = self
            .fetch_items(&query, "2000")
            .await?;
        items.retain(|item| {
            item.is_movie()
                && item
                    .url()
                    .is_some()
        });
        if items
            .iter()
            .any(|item| item_matches_title(item, &search))
        {
            items.retain(|item| item_matches_title(item, &search));
        }
        items.sort_by(|a, b| {
            score_item(b, &search)
                .cmp(&score_item(a, &search))
                .then_with(|| {
                    b.seeders
                        .cmp(&a.seeders)
                })
                .then_with(|| {
                    b.peers
                        .cmp(&a.peers)
                })
                .then_with(|| {
                    a.size
                        .unwrap_or(i64::MAX)
                        .cmp(
                            &b.size
                                .unwrap_or(i64::MAX),
                        )
                })
        });

        Ok(items
            .into_iter()
            .take(MAX_RESULTS)
            .filter_map(|item| {
                Some(crate::stream::StreamInfo {
                    descriptor: magnet_to_descriptor(&item.url()?, None)?,
                    name: Some(item.label(&self.name)),
                    ..Default::default()
                })
            })
            .collect())
    }

    async fn resolve_episode(
        &self,
        media: &db::Media,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        let season = media
            .parent_idx
            .unwrap_or(1);
        let episode = media
            .idx
            .unwrap_or(1);
        let se = format!("S{:02}E{:02}", season, episode);
        let query = format!("{} {}", media.title, se);

        tracing::debug!(query, title = %media.title, %se, "torznab episode stream lookup");

        let search = MediaSearch {
            title: media
                .title
                .clone(),
            extra: Some(se.clone()),
            title_tokens: significant_tokens(&media.title),
            extra_tokens: vec![se.to_ascii_lowercase()],
        };

        let mut items = self
            .fetch_items(&query, "5000")
            .await?;
        items.retain(|item| {
            item.is_episode()
                && item
                    .url()
                    .is_some()
        });
        if items
            .iter()
            .any(|item| item_matches_title(item, &search))
        {
            items.retain(|item| item_matches_title(item, &search));
        }
        items.sort_by(|a, b| {
            score_item(b, &search)
                .cmp(&score_item(a, &search))
                .then_with(|| {
                    b.seeders
                        .cmp(&a.seeders)
                })
                .then_with(|| {
                    b.peers
                        .cmp(&a.peers)
                })
                .then_with(|| {
                    a.size
                        .unwrap_or(i64::MAX)
                        .cmp(
                            &b.size
                                .unwrap_or(i64::MAX),
                        )
                })
        });

        Ok(items
            .into_iter()
            .take(MAX_RESULTS)
            .filter_map(|item| {
                Some(crate::stream::StreamInfo {
                    descriptor: magnet_to_descriptor(&item.url()?, None)?,
                    name: Some(item.label(&self.name)),
                    ..Default::default()
                })
            })
            .collect())
    }

    async fn fetch_items(&self, query: &str, cat: &str) -> Result<Vec<TorznabItem>> {
        let search_url = format!(
            "{}?t=search&q={}&cat={}&limit={}",
            self.url,
            urlencoding::encode(query),
            cat,
            MAX_CANDIDATES
        );

        let resp = self
            .client
            .get(&search_url)
            .send()
            .await?;
        if !resp
            .status()
            .is_success()
        {
            let status = resp.status();
            let body = resp
                .text()
                .await
                .unwrap_or_default();
            bail!("torznab HTTP {} - {}", status, body);
        }

        let body = resp
            .text()
            .await?;
        parse_torznab_items(body.as_bytes())
    }
}

#[derive(Debug, Clone)]
struct MediaSearch {
    title: String,
    extra: Option<String>,
    title_tokens: Vec<String>,
    extra_tokens: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct TorznabItem {
    title: String,
    magnet_url: Option<String>,
    enclosure_url: Option<String>,
    category: Option<String>,
    seeders: i64,
    peers: i64,
    size: Option<i64>,
}

impl TorznabItem {
    fn category_prefix(&self) -> Option<&str> {
        self.category
            .as_deref()
    }

    fn is_music(&self) -> bool {
        self.category_prefix()
            .map(|c| {
                c.eq_ignore_ascii_case("music") || c == "3000" || c.starts_with("30")
            })
            .unwrap_or(true)
    }

    fn is_movie(&self) -> bool {
        self.category_prefix()
            .map(|c| c == "2000" || c.starts_with("20"))
            .unwrap_or(true)
    }

    fn is_episode(&self) -> bool {
        self.category_prefix()
            .map(|c| c == "5000" || c.starts_with("50"))
            .unwrap_or(true)
    }

    fn url(&self) -> Option<String> {
        self.magnet_url
            .clone()
            .or_else(|| {
                self.enclosure_url
                    .clone()
            })
            .filter(|u| u.starts_with("magnet:"))
    }

    fn label(&self, instance_name: &str) -> String {
        let quality = infer_quality(&self.title);
        let size = self
            .size
            .map(format_size);
        let mut label = format!("Torznab - {}", self.title);
        if let Some(q) = quality {
            label.push_str(&format!(" - {q}"));
        }
        if let Some(s) = size {
            label.push_str(&format!(" - {s}"));
        }
        label.push_str(&format!(" - {instance_name}"));
        label
    }
}

fn score_item(item: &TorznabItem, search: &MediaSearch) -> i64 {
    let norm = normalize_for_match(&item.title);
    let mut score = 0i64;

    if contains_all_tokens(&norm, &search.title_tokens) {
        score += 1000;
    } else {
        score -= 1000;
    }

    if !search
        .extra_tokens
        .is_empty()
        && contains_all_tokens(&norm, &search.extra_tokens)
    {
        score += 250;
    }

    // penalise collection-style results
    if norm.contains("discography") {
        score -= 500;
    }
    if norm.contains("complete")
        || norm.contains("collection")
        || norm.contains("trilogy")
    {
        score -= 250;
    }
    if norm.contains("album") || norm.contains("albums") {
        score -= 150;
    }
    if norm.contains("season pack") || norm.contains("complete series") {
        score -= 400;
    }

    score
        + item
            .seeders
            .saturating_mul(10)
        + item.peers
}

fn item_matches_title(item: &TorznabItem, search: &MediaSearch) -> bool {
    contains_all_tokens(&normalize_for_match(&item.title), &search.title_tokens)
}

fn parse_torznab_items(bytes: &[u8]) -> Result<Vec<TorznabItem>> {
    let mut reader = Reader::from_reader(bytes);
    reader
        .config_mut()
        .trim_text(true);

    let mut buf = Vec::new();
    let mut items = Vec::new();
    let mut item = None::<TorznabItem>;
    let mut current_field = None::<Field>;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => match e
                .name()
                .as_ref()
            {
                b"item" => item = Some(TorznabItem::default()),
                b"title" if item.is_some() => current_field = Some(Field::Title),
                b"category" if item.is_some() => current_field = Some(Field::Category),
                b"size" if item.is_some() => current_field = Some(Field::Size),
                b"enclosure" if item.is_some() => {
                    if let Some(cur) = item.as_mut() {
                        for attr in e
                            .attributes()
                            .with_checks(false)
                        {
                            let attr = attr?;
                            if attr
                                .key
                                .as_ref()
                                == b"url"
                            {
                                cur.enclosure_url = Some(
                                    attr.decode_and_unescape_value(reader.decoder())?
                                        .into_owned(),
                                );
                            }
                        }
                    }
                }
                b"torznab:attr" if item.is_some() => {
                    apply_torznab_attr(&reader, &mut item, e.attributes())?;
                }
                _ => {}
            },
            Event::Empty(e) => match e
                .name()
                .as_ref()
            {
                b"enclosure" if item.is_some() => {
                    if let Some(cur) = item.as_mut() {
                        for attr in e
                            .attributes()
                            .with_checks(false)
                        {
                            let attr = attr?;
                            if attr
                                .key
                                .as_ref()
                                == b"url"
                            {
                                cur.enclosure_url = Some(
                                    attr.decode_and_unescape_value(reader.decoder())?
                                        .into_owned(),
                                );
                            }
                        }
                    }
                }
                b"torznab:attr" if item.is_some() => {
                    apply_torznab_attr(&reader, &mut item, e.attributes())?;
                }
                _ => {}
            },
            Event::Text(e) => {
                if let (Some(cur), Some(field)) =
                    (item.as_mut(), current_field.as_ref())
                {
                    let text = e
                        .unescape()?
                        .into_owned();
                    match field {
                        Field::Title => cur.title = text,
                        Field::Category => cur.category = Some(text),
                        Field::Size => {
                            cur.size = text
                                .parse()
                                .ok()
                        }
                    }
                }
            }
            Event::End(e) => match e
                .name()
                .as_ref()
            {
                b"item" => {
                    if let Some(cur) = item.take() {
                        items.push(cur);
                    }
                }
                b"title" | b"category" | b"size" => current_field = None,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(items)
}

fn apply_torznab_attr<'a>(
    reader: &Reader<&[u8]>,
    item: &mut Option<TorznabItem>,
    mut attributes: quick_xml::events::attributes::Attributes<'a>,
) -> Result<()> {
    let mut name = None;
    let mut value = None;

    for attr in attributes.with_checks(false) {
        let attr = attr?;
        match attr
            .key
            .as_ref()
        {
            b"name" => {
                name = Some(
                    attr.decode_and_unescape_value(reader.decoder())?
                        .into_owned(),
                )
            }
            b"value" => {
                value = Some(
                    attr.decode_and_unescape_value(reader.decoder())?
                        .into_owned(),
                )
            }
            _ => {}
        }
    }

    let Some(cur) = item.as_mut() else {
        return Ok(());
    };
    let (Some(name), Some(value)) = (name, value) else {
        return Ok(());
    };

    match name.as_str() {
        "magneturl" => cur.magnet_url = Some(value),
        "category" => cur.category = Some(value),
        "seeders" => {
            cur.seeders = value
                .parse()
                .unwrap_or(0)
        }
        "peers" => {
            cur.peers = value
                .parse()
                .unwrap_or(0)
        }
        "size" => {
            cur.size = value
                .parse()
                .ok()
        }
        _ => {}
    }

    Ok(())
}

#[derive(Debug)]
enum Field {
    Title,
    Category,
    Size,
}

#[derive(Debug, Clone, Copy)]
enum AudioFormatHint {
    Flac,
    Mp3,
    Aac,
    Opus,
    Vorbis,
}

impl AudioFormatHint {
    fn mime_type(self) -> &'static str {
        match self {
            Self::Flac => "audio/flac",
            Self::Mp3 => "audio/mpeg",
            Self::Aac => "audio/mp4",
            Self::Opus | Self::Vorbis => "audio/webm",
        }
    }

    fn codec(self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Aac => "aac",
            Self::Opus => "opus",
            Self::Vorbis => "vorbis",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Flac => "FLAC",
            Self::Mp3 => "MP3",
            Self::Aac => "AAC",
            Self::Opus => "Opus",
            Self::Vorbis => "Vorbis",
        }
    }
}

fn infer_audio_format(title: &str) -> Option<AudioFormatHint> {
    let lower = title.to_ascii_lowercase();
    if lower.contains("flac") {
        Some(AudioFormatHint::Flac)
    } else if lower.contains("opus") {
        Some(AudioFormatHint::Opus)
    } else if lower.contains("vorbis") || lower.contains("ogg") {
        Some(AudioFormatHint::Vorbis)
    } else if lower.contains("aac") || lower.contains("m4a") {
        Some(AudioFormatHint::Aac)
    } else if lower.contains("mp3") || lower.contains("320") {
        Some(AudioFormatHint::Mp3)
    } else {
        None
    }
}

fn infer_video_quality(title: &str) -> Option<&'static str> {
    let lower = title.to_ascii_lowercase();
    if lower.contains("2160p") || lower.contains("4k") || lower.contains("uhd") {
        Some("4K")
    } else if lower.contains("1080p") {
        Some("1080p")
    } else if lower.contains("720p") {
        Some("720p")
    } else if lower.contains("480p") {
        Some("480p")
    } else {
        None
    }
}

fn infer_quality(title: &str) -> Option<&'static str> {
    infer_audio_format(title)
        .map(AudioFormatHint::label)
        .or_else(|| {
            let lower = title.to_ascii_lowercase();
            if lower.contains("320") {
                Some("MP3 320")
            } else {
                None
            }
        })
        .or_else(|| infer_video_quality(title))
}

fn significant_tokens(input: &str) -> Vec<String> {
    normalize_for_match(input)
        .split_whitespace()
        .filter(|t| t.len() > 1)
        .filter(|t| {
            !matches!(
                *t,
                "a" | "an"
                    | "and"
                    | "by"
                    | "feat"
                    | "ft"
                    | "in"
                    | "of"
                    | "on"
                    | "remaster"
                    | "remastered"
                    | "the"
                    | "with"
            )
        })
        .map(str::to_string)
        .collect()
}

fn normalize_for_match(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn contains_all_tokens(haystack: &str, tokens: &[String]) -> bool {
    !tokens.is_empty()
        && tokens
            .iter()
            .all(|t| haystack.contains(t.as_str()))
}

fn magnet_to_descriptor(
    magnet: &str,
    file_hint: Option<String>,
) -> Option<crate::stream::StreamDescriptor> {
    let parsed = url::Url::parse(magnet).ok()?;
    let info_hash = parsed
        .query_pairs()
        .find(|(k, _)| k == "xt")
        .and_then(|(_, v)| {
            v.strip_prefix("urn:btih:")
                .map(|h| h.to_ascii_lowercase())
        })?;
    let trackers = parsed
        .query_pairs()
        .filter(|(k, _)| k == "tr")
        .map(|(_, v)| v.into_owned())
        .collect();
    Some(crate::stream::StreamDescriptor::Torrent {
        info_hash,
        file_hint,
        file_idx: None,
        trackers,
    })
}

fn format_size(bytes: i64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else {
        format!("{:.0} MiB", b / MIB)
    }
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("remux-server/1.0")
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .expect("failed to build HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_torznab_items() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <item>
      <title>Artist - Track [FLAC]</title>
      <category>music</category>
      <size>104857600</size>
      <enclosure url="magnet:?xt=urn:btih:abc&amp;dn=Artist+-+Track" length="104857600" type="application/x-bittorrent;x-scheme-handler/magnet"></enclosure>
      <torznab:attr name="magneturl" value="magnet:?xt=urn:btih:def&amp;dn=Artist+-+Track"></torznab:attr>
      <torznab:attr name="seeders" value="7"></torznab:attr>
      <torznab:attr name="peers" value="9"></torznab:attr>
      <torznab:attr name="category" value="3000"></torznab:attr>
    </item>
  </channel>
</rss>"#;

        let items = parse_torznab_items(xml).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Artist - Track [FLAC]");
        assert_eq!(items[0].seeders, 7);
        assert_eq!(items[0].peers, 9);
        assert_eq!(
            items[0]
                .category
                .as_deref(),
            Some("3000")
        );
        assert_eq!(
            items[0]
                .url()
                .as_deref(),
            Some("magnet:?xt=urn:btih:def&dn=Artist+-+Track")
        );
        assert!(items[0].is_music());
    }

    #[test]
    fn ranks_track_title_matches_and_appends_file_hint() {
        let search = MediaSearch {
            title: "Hit Em Up".to_string(),
            extra: Some("2Pac".to_string()),
            title_tokens: significant_tokens("Hit Em Up"),
            extra_tokens: significant_tokens("2Pac"),
        };
        let track = TorznabItem {
            title: "2Pac - Hit Em Up [FLAC]".to_string(),
            seeders: 2,
            peers: 2,
            ..Default::default()
        };
        let album = TorznabItem {
            title: "2Pac - Discography [FLAC Songs]".to_string(),
            seeders: 200,
            peers: 200,
            ..Default::default()
        };

        assert!(item_matches_title(&track, &search));
        assert!(!item_matches_title(&album, &search));
        assert!(score_item(&track, &search) > score_item(&album, &search));
        let descriptor = magnet_to_descriptor(
            "magnet:?xt=urn:btih:abc&dn=2Pac",
            Some("Hit Em Up".to_string()),
        );
        assert!(matches!(
            descriptor,
            Some(crate::stream::StreamDescriptor::Torrent {
                ref info_hash,
                file_hint: Some(ref hint),
                ..
            }) if info_hash == "abc" && hint == "Hit Em Up"
        ));
    }

    #[test]
    fn category_detection() {
        let movie = TorznabItem {
            category: Some("2030".to_string()),
            ..Default::default()
        };
        let episode = TorznabItem {
            category: Some("5040".to_string()),
            ..Default::default()
        };
        let music = TorznabItem {
            category: Some("3010".to_string()),
            ..Default::default()
        };
        assert!(movie.is_movie());
        assert!(!movie.is_episode());
        assert!(episode.is_episode());
        assert!(!episode.is_movie());
        assert!(music.is_music());
        assert!(!music.is_movie());
    }

    #[test]
    fn video_quality_inference() {
        assert_eq!(
            infer_video_quality("Movie.2023.1080p.BluRay.x265"),
            Some("1080p")
        );
        assert_eq!(infer_video_quality("Show.S01E01.4K.HDR"), Some("4K"));
        assert_eq!(infer_video_quality("Movie.720p.WEB-DL"), Some("720p"));
        assert_eq!(infer_video_quality("Movie.BluRay.x264"), None);
    }
}
