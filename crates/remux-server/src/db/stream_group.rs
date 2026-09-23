use anyhow::Result;
use chrono::Utc;
use remux_sdks::remux::{
    FilterMatchMode, NumericOp, SetOp, StreamCodec, StreamFilter, StreamQuality,
    StreamResolution, StreamRule, format_bitrate_rule, format_size_rule,
    language_label, normalize_lang_code,
};
use sqlx::SqlitePool;
use std::collections::HashSet;
use uuid::Uuid;

use crate::{
    api::MediaStreamType,
    db::{Media, Settings, StreamGroupData},
    stream::StreamInfo,
};

const LEGACY_SETTINGS_KEY: &str = "remux:stream_groups";

#[derive(Debug, Clone)]
pub struct StreamGroup {
    pub id: Uuid,
    pub name: String,
    pub filter: StreamFilter,
    pub priority: i64,
    pub enabled: bool,
    pub hidden: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchOutcome {
    Match,
    PassThrough,
    NoMatch,
}

fn bool_to_outcome(hit: bool) -> MatchOutcome {
    if hit {
        MatchOutcome::Match
    } else {
        MatchOutcome::NoMatch
    }
}

impl StreamGroup {
    pub fn display_name(&self) -> String {
        if !self
            .name
            .is_empty()
        {
            return self
                .name
                .clone();
        }
        auto_name(&self.filter)
    }

    fn from_media(m: Media) -> Option<Self> {
        let data = m.stream_group_data?;
        Some(Self {
            id: m.id,
            name: data.name,
            filter: data.filter,
            priority: data.priority,
            enabled: m.enabled,
            hidden: data.hidden,
            created_at: data.created_at,
        })
    }

    async fn save(&self, db: &SqlitePool) -> Result<()> {
        let data = StreamGroupData {
            name: self
                .name
                .clone(),
            filter: self
                .filter
                .clone(),
            priority: self.priority,
            hidden: self.hidden,
            created_at: self
                .created_at
                .clone(),
        };
        let data_json = serde_json::to_string(&data)?;
        let now = Utc::now().naive_utc();
        sqlx::query(
            "INSERT INTO media (id, kind, title, enabled, stream_group_data, external_ids, locked_fields, created_at, updated_at)
             VALUES (?, 'stream_group', ?, ?, ?, '{}', '[]', ?, ?)
             ON CONFLICT (id) DO UPDATE SET
                 title = excluded.title,
                 enabled = excluded.enabled,
                 stream_group_data = excluded.stream_group_data,
                 external_ids = COALESCE(media.external_ids, excluded.external_ids),
                 locked_fields = COALESCE(media.locked_fields, '[]'),
                 updated_at = excluded.updated_at",
        )
        .bind(self.id)
        .bind(self.display_name())
        .bind(self.enabled)
        .bind(&data_json)
        .bind(now)
        .bind(now)
        .execute(db)
        .await?;
        Ok(())
    }

    /// One-time migration: reads groups from the legacy settings key and saves them
    /// as media rows. After migration, removes the settings key.
    pub async fn migrate_from_settings(db: &SqlitePool) {
        #[derive(serde::Deserialize)]
        struct Legacy {
            id: Uuid,
            #[serde(default)]
            name: String,
            filter: StreamFilter,
            #[serde(default)]
            priority: i64,
            #[serde(default = "default_true")]
            enabled: bool,
            #[serde(default)]
            hidden: bool,
            #[serde(default)]
            created_at: String,
        }
        fn default_true() -> bool {
            true
        }

        let json = match Settings::get(db, LEGACY_SETTINGS_KEY).await {
            Ok(Some(j)) => j,
            _ => return,
        };
        let legacy: Vec<Legacy> = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(_) => return,
        };
        for l in legacy {
            let group = StreamGroup {
                id: l.id,
                name: l.name,
                filter: l.filter,
                priority: l.priority,
                enabled: l.enabled,
                hidden: l.hidden,
                created_at: l.created_at,
            };
            let _ = group
                .save(db)
                .await;
        }
        let _ = sqlx::query("DELETE FROM settings WHERE key = ?")
            .bind(LEGACY_SETTINGS_KEY)
            .execute(db)
            .await;
    }

    pub async fn list(db: &SqlitePool) -> Result<Vec<Self>> {
        let rows: Vec<Media> =
            sqlx::query_as("SELECT * FROM media WHERE kind = 'stream_group'")
                .fetch_all(db)
                .await?;
        let mut groups: Vec<Self> = rows
            .into_iter()
            .filter_map(Self::from_media)
            .collect();
        groups.sort_by_key(|g| g.priority);
        Ok(groups)
    }

    pub async fn get_by_id(db: &SqlitePool, id: &Uuid) -> Result<Option<Self>> {
        let row: Option<Media> = sqlx::query_as(
            "SELECT * FROM media WHERE id = ? AND kind = 'stream_group'",
        )
        .bind(id)
        .fetch_optional(db)
        .await?;
        Ok(row.and_then(Self::from_media))
    }

    pub async fn create(
        db: &SqlitePool,
        name: &str,
        filter: StreamFilter,
        priority: i64,
    ) -> Result<Self> {
        let group = Self {
            id: Uuid::new_v4(),
            name: name.to_string(),
            filter,
            priority,
            enabled: true,
            hidden: false,
            created_at: Utc::now()
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
        };
        group
            .save(db)
            .await?;
        Ok(group)
    }

    pub async fn update(
        db: &SqlitePool,
        id: &Uuid,
        name: &str,
        filter: StreamFilter,
        priority: i64,
        enabled: bool,
        hidden: bool,
    ) -> Result<Self> {
        let existing = Self::get_by_id(db, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("stream group not found"))?;
        let group = Self {
            id: *id,
            name: name.to_string(),
            filter,
            priority,
            enabled,
            hidden,
            created_at: existing.created_at,
        };
        group
            .save(db)
            .await?;
        Ok(group)
    }

    pub async fn delete(db: &SqlitePool, id: &Uuid) -> Result<()> {
        Media::delete(db, id).await
    }

    /// Filter a list of media sources by the enabled stream groups.
    ///
    /// Returns one representative source per matching group (the first
    /// match by priority order), plus any unmatched sources when
    /// `show_ungrouped` is true.  When no groups are enabled the original
    /// list is returned unchanged.
    pub async fn filter_sources(
        db: &SqlitePool,
        sources: Vec<Media>,
        show_ungrouped: bool,
    ) -> Vec<Media> {
        let groups = match Self::list(db).await {
            Ok(g) => g,
            Err(_) => return sources,
        };
        let enabled: Vec<&StreamGroup> = groups
            .iter()
            .filter(|g| g.enabled)
            .collect();
        if enabled.is_empty() {
            return sources;
        }

        let mut result: Vec<Media> = vec![];
        let mut matched_ids: HashSet<Uuid> = HashSet::new();

        for group in &enabled {
            let matching: Vec<&Media> = sources
                .iter()
                .filter(|s| {
                    s.stream_info
                        .as_ref()
                        .map_or(false, |info| {
                            group.match_stream(
                                info,
                                s.probe_data
                                    .as_ref(),
                                s.runtime,
                            ) == MatchOutcome::Match
                        })
                })
                .collect();

            if matching.is_empty() {
                continue;
            }

            for s in &matching {
                matched_ids.insert(s.id);
            }

            if !group.hidden {
                // Only the first (highest-priority) match is shown.
                // group_id marks it as a group representative; the real stream
                // UUID stays in best.id so internal probe URLs stay correct.
                let mut best = matching[0].clone();
                best.title = group.display_name();
                best.group_id = Some(group.id);
                result.push(best);
            }
        }

        if show_ungrouped {
            for s in &sources {
                if !matched_ids.contains(&s.id) {
                    result.push(s.clone());
                }
            }
        }

        result
    }
}

/// Filter a list of source Media items using a `StreamFilter`.
/// Sources without `stream_info` (unparseable filename) are kept unchanged.
/// An empty filter (no rules) is a no-op.
pub fn apply_stream_filter(filter: &StreamFilter, sources: Vec<Media>) -> Vec<Media> {
    if filter
        .rules
        .is_empty()
    {
        return sources;
    }
    let temp = StreamGroup {
        id: Uuid::nil(),
        name: String::new(),
        filter: filter.clone(),
        priority: 0,
        enabled: true,
        hidden: false,
        created_at: String::new(),
    };
    sources
        .into_iter()
        .filter(|s| {
            s.stream_info
                .as_ref()
                .map_or(true, |info| {
                    temp.match_stream(
                        info,
                        s.probe_data
                            .as_ref(),
                        s.runtime,
                    ) != MatchOutcome::NoMatch
                })
        })
        .collect()
}

impl StreamGroup {
    #[cfg(test)]
    pub fn match_outcome(
        &self,
        info: &StreamInfo,
        probe_data: Option<&crate::api::MediaSourceInfo>,
    ) -> MatchOutcome {
        self.match_stream(info, probe_data, None)
    }

    /// `runtime_secs` feeds the Bitrate rule's size ÷ runtime estimate.
    pub fn match_stream(
        &self,
        info: &StreamInfo,
        probe_data: Option<&crate::api::MediaSourceInfo>,
        runtime_secs: Option<i64>,
    ) -> MatchOutcome {
        let filter = &self.filter;
        if filter
            .rules
            .is_empty()
        {
            return MatchOutcome::Match;
        }

        let Some((resolution, source, codec)) = detect_stream_quality(info) else {
            return MatchOutcome::PassThrough;
        };

        let eval = |rule: &StreamRule| -> MatchOutcome {
            match rule {
                StreamRule::Resolution { op, values } => {
                    if resolution == StreamResolution::Other
                        && !values.contains(&StreamResolution::Other)
                    {
                        return MatchOutcome::PassThrough;
                    }
                    let hit = values.contains(&resolution);
                    bool_to_outcome(matches!(op, SetOp::In | SetOp::Is) == hit)
                }
                StreamRule::Quality { op, values } => {
                    if source == StreamQuality::Other
                        && !values.contains(&StreamQuality::Other)
                    {
                        return MatchOutcome::PassThrough;
                    }
                    let hit = values.contains(&source);
                    bool_to_outcome(matches!(op, SetOp::In | SetOp::Is) == hit)
                }
                StreamRule::Codec { op, values } => {
                    if codec == StreamCodec::Other
                        && !values.contains(&StreamCodec::Other)
                    {
                        return MatchOutcome::PassThrough;
                    }
                    let hit = values.contains(&codec);
                    bool_to_outcome(matches!(op, SetOp::In | SetOp::Is) == hit)
                }
                // No probe data → PassThrough: we can't confirm or deny the
                // language, so the source must not be absorbed by the group
                // (which shows only one representative) but also must not be
                // hidden — it stays visible via the ungrouped list.
                StreamRule::AudioLanguage { op, values } => match probe_data {
                    None => MatchOutcome::PassThrough,
                    Some(pd) => {
                        let wanted: Vec<String> = values
                            .iter()
                            .map(|v| normalize_lang_code(v))
                            .collect();
                        let hit = pd
                            .media_streams
                            .iter()
                            .any(|s| {
                                s.type_ == Some(MediaStreamType::Audio)
                                    && s.language
                                        .as_deref()
                                        .map_or(false, |l| {
                                            let l = normalize_lang_code(l);
                                            !matches!(
                                                l.as_str(),
                                                "und" | "mis" | "zxx" | "mul"
                                            ) && wanted.contains(&l)
                                        })
                            });
                        bool_to_outcome(matches!(op, SetOp::In | SetOp::Is) == hit)
                    }
                },
                // Unknown size (None) passes through, consistent with Size's
                // original semantics and the AudioLanguage behavior above.
                StreamRule::Size { op, value } => match info.size {
                    None => MatchOutcome::PassThrough,
                    Some(s) => bool_to_outcome(match op {
                        NumericOp::Eq => s == *value,
                        NumericOp::NotEq => s != *value,
                        NumericOp::Gt => s > *value,
                        NumericOp::Lt => s < *value,
                    }),
                },
                StreamRule::Bitrate { op, value } => {
                    let bitrate = probe_data
                        .and_then(|p| p.bitrate)
                        .or_else(|| {
                            let secs = runtime_secs.filter(|s| *s > 0)?;
                            Some(info.size? * 8 / secs)
                        });
                    match bitrate {
                        None => MatchOutcome::PassThrough,
                        Some(b) => bool_to_outcome(match op {
                            NumericOp::Eq => b == *value,
                            NumericOp::NotEq => b != *value,
                            NumericOp::Gt => b > *value,
                            NumericOp::Lt => b < *value,
                        }),
                    }
                }
                // Unknown addon (None) passes through, consistent with Size/
                // AudioLanguage above.
                StreamRule::Addon { op, values } => match info.addon_id {
                    None => MatchOutcome::PassThrough,
                    Some(id) => {
                        let hit = values.contains(&id);
                        bool_to_outcome(matches!(op, SetOp::In | SetOp::Is) == hit)
                    }
                },
            }
        };

        let outcomes: Vec<MatchOutcome> = filter
            .rules
            .iter()
            .map(eval)
            .collect();
        match filter.match_mode {
            FilterMatchMode::All => {
                if outcomes
                    .iter()
                    .any(|o| matches!(o, MatchOutcome::NoMatch))
                {
                    MatchOutcome::NoMatch
                } else if outcomes
                    .iter()
                    .any(|o| matches!(o, MatchOutcome::PassThrough))
                {
                    MatchOutcome::PassThrough
                } else {
                    MatchOutcome::Match
                }
            }
            FilterMatchMode::Any => {
                if outcomes
                    .iter()
                    .any(|o| matches!(o, MatchOutcome::Match))
                {
                    MatchOutcome::Match
                } else {
                    MatchOutcome::NoMatch
                }
            }
        }
    }

    fn candidates_for_group(group: &StreamGroup, raw_sources: &[Media]) -> Vec<Media> {
        let mut v: Vec<Media> = raw_sources
            .iter()
            .filter(|s| {
                s.stream_info
                    .as_ref()
                    .map_or(false, |info| {
                        group.match_stream(
                            info,
                            s.probe_data
                                .as_ref(),
                            s.runtime,
                        ) == MatchOutcome::Match
                    })
            })
            .cloned()
            .collect();
        v.sort_by_key(|s| {
            s.idx
                .unwrap_or(0)
        });
        v
    }

    /// Returns all raw sources matching this group for the given parent item,
    /// sorted by idx. Used by PlaybackInfo for group-scoped probe fallback.
    pub async fn streams_for(
        db: &SqlitePool,
        group_id: &Uuid,
        parent_id: &Uuid,
    ) -> Result<Vec<Media>> {
        let group = Self::get_by_id(db, group_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("stream group not found"))?;
        let mut parent = Media::get_by_id(db, parent_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("parent item not found"))?;
        let raw = parent
            .streams(db)
            .await?;
        let raw = if raw.is_empty() { vec![parent] } else { raw };
        Ok(Self::candidates_for_group(&group, &raw))
    }

    /// Returns streams from all enabled groups that come after `current_group_id`
    /// in priority order, concatenated. Used to extend the probe fallback pool
    /// so that exhausting one group cascades into the next.
    pub async fn streams_for_groups_after(
        db: &SqlitePool,
        current_group_id: &Uuid,
        parent_id: &Uuid,
    ) -> Result<Vec<Media>> {
        let groups = Self::list(db).await?;
        let mut parent = Media::get_by_id(db, parent_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("parent item not found"))?;
        let raw = parent
            .streams(db)
            .await?;
        let raw = if raw.is_empty() { vec![parent] } else { raw };

        let mut result = vec![];
        let mut found = false;
        for group in groups
            .iter()
            .filter(|g| g.enabled)
        {
            if !found {
                if group.id == *current_group_id {
                    found = true;
                }
                continue;
            }
            result.extend(Self::candidates_for_group(group, &raw));
        }
        Ok(result)
    }
}

/// Picks the lowest of hunch's screen-size candidates.
///
/// Filenames like "UHD BluRay 1080p ... x265" parse to multiple screen_size
/// hits (hunch maps the bare word "UHD" to 2160p in addition to the literal
/// 1080p token) because "UHD BluRay" credits the source disc, not the encode
/// resolution. Encoders only ever downscale from source, never label a
/// downscaled encode with a higher resolution, so the smaller candidate is
/// always the real encode resolution when they disagree.
/// Parse a stream's filename/name into (resolution, quality source, codec)
/// hints, the same way group filter matching does. Returns `None` when there
/// is nothing to parse (no filename/name at all, or hunch found nothing
/// usable) — callers treat that as "unknown", not "worst".
pub(crate) fn detect_stream_quality(
    info: &StreamInfo,
) -> Option<(StreamResolution, StreamQuality, StreamCodec)> {
    let raw = info
        .filename
        .as_deref()
        .or(info
            .name
            .as_deref())?;

    let candidates: Vec<&str> = if raw.contains('\n') {
        raw.lines()
            .filter(|l| {
                !l.trim()
                    .is_empty()
            })
            .collect()
    } else {
        vec![raw]
    };

    let best = candidates
        .iter()
        .map(|s| hunch::hunch(s))
        .max_by_key(|p| {
            (p.screen_size()
                .is_some() as u8)
                + (p.source()
                    .is_some() as u8)
        })?;

    let resolution = min_screen_size(&best)
        .and_then(StreamResolution::from_hunch)
        .unwrap_or(StreamResolution::Other);
    let source = {
        let s = canonical_source(&best);
        if s == StreamQuality::Other {
            fallback_source(raw)
        } else {
            s
        }
    };
    let codec = best
        .video_codec()
        .and_then(StreamCodec::from_hunch)
        .unwrap_or(StreamCodec::Other);

    Some((resolution, source, codec))
}

/// Coarse pre-probe "how good does this look" weight — higher is better.
/// Used only to choose probe attempt order before real technical specs are
/// available. It must not be used to mutate the persisted addon source order.
pub(crate) trait PreProbeQualityExt {
    fn quality_weight(&self) -> (u8, u8);
}

/// Release-source weight — higher is better — shared by the pre-probe
/// candidate ordering (`quality_weight`) and the post-probe capability rank
/// (`device_profile::capability_rank`), so "remux beats BluRay beats WEB-DL"
/// means the same thing in both places.
pub(crate) fn source_quality_weight(quality: &StreamQuality) -> u8 {
    match quality {
        StreamQuality::BluRayRemux => 6,
        StreamQuality::BluRay => 5,
        StreamQuality::WebDl => 4,
        StreamQuality::WebRip => 3,
        StreamQuality::Hdtv => 2,
        StreamQuality::Dvd => 1,
        StreamQuality::Tv | StreamQuality::Other => 0,
    }
}

fn resolution_weight(resolution: &StreamResolution) -> u8 {
    match resolution {
        StreamResolution::R2160p => 5,
        StreamResolution::R1080p => 4,
        StreamResolution::R720p => 3,
        StreamResolution::R480p => 2,
        StreamResolution::R360p => 1,
        StreamResolution::Other => 0,
    }
}

/// Same filename-derived source-quality weight as `quality_weight`, usable
/// wherever only a `StreamInfo` (not a full `Media` row) is on hand — e.g.
/// scoring an already-probed `MediaSourceInfo` in `device_profile.rs`.
pub(crate) fn detect_source_quality_weight(info: Option<&StreamInfo>) -> u8 {
    info.and_then(detect_stream_quality)
        .map(|(_resolution, quality, _codec)| source_quality_weight(&quality))
        .unwrap_or(0)
}

impl PreProbeQualityExt for Media {
    fn quality_weight(&self) -> (u8, u8) {
        let Some((resolution, quality, _codec)) = self
            .stream_info
            .as_ref()
            .and_then(detect_stream_quality)
        else {
            return (0, 0);
        };
        (
            resolution_weight(&resolution),
            source_quality_weight(&quality),
        )
    }
}

pub(crate) fn min_screen_size<'a>(parsed: &'a hunch::HunchResult) -> Option<&'a str> {
    parsed
        .all(hunch::Property::ScreenSize)
        .into_iter()
        .min_by_key(|s| {
            s.trim_end_matches(['p', 'i'])
                .parse::<u32>()
                .unwrap_or(u32::MAX)
        })
}

fn auto_name(filter: &StreamFilter) -> String {
    let parts: Vec<String> = filter
        .rules
        .iter()
        .filter_map(|r| {
            let labels: Vec<String> = match r {
                StreamRule::Resolution { values, .. } => values
                    .iter()
                    .map(|v| v.label())
                    .map(str::to_owned)
                    .collect(),
                StreamRule::Quality { values, .. } => values
                    .iter()
                    .map(|v| v.label())
                    .map(str::to_owned)
                    .collect(),
                StreamRule::Codec { values, .. } => values
                    .iter()
                    .map(|v| v.label())
                    .map(str::to_owned)
                    .collect(),
                StreamRule::AudioLanguage { values, .. } => values
                    .iter()
                    .map(|c| language_label(c))
                    .collect(),
                StreamRule::Size { op, value } => vec![format_size_rule(*op, *value)],
                StreamRule::Bitrate { op, value } => {
                    vec![format_bitrate_rule(*op, *value)]
                }
                StreamRule::Addon { values, .. } => {
                    if values.len() == 1 {
                        vec!["1 addon".to_string()]
                    } else {
                        vec![format!("{} addons", values.len())]
                    }
                }
            };
            if labels.is_empty() {
                None
            } else {
                Some(labels.join("/"))
            }
        })
        .collect();
    if parts.is_empty() {
        "All streams".to_string()
    } else {
        parts.join(" · ")
    }
}

fn canonical_source(parsed: &hunch::HunchResult) -> StreamQuality {
    let other = parsed.other();
    let is_remux = other.contains(&"Remux");
    let is_rip = other.contains(&"Rip");
    let Some(source) = parsed.source() else {
        // No source keyword — remux is always Blu-ray regardless
        return if is_remux {
            StreamQuality::BluRayRemux
        } else {
            StreamQuality::Other
        };
    };
    match source {
        "Web" if is_rip => StreamQuality::WebRip,
        "Web" => StreamQuality::WebDl,
        "Blu-ray" | "Ultra HD Blu-ray" if is_remux => StreamQuality::BluRayRemux,
        "Blu-ray" | "Ultra HD Blu-ray" => StreamQuality::BluRay,
        "HDTV" => StreamQuality::Hdtv,
        "DVD" => StreamQuality::Dvd,
        "TV" => StreamQuality::Tv,
        _ => StreamQuality::Other,
    }
}

/// Fallback source detection via case-insensitive substring search.
/// Used when hunch fails to identify the source (e.g. "WEBRip-AVC" or
/// space-delimited multi-language filenames where hunch loses the token).
fn fallback_source(raw: &str) -> StreamQuality {
    let lower = raw.to_lowercase();
    if lower.contains("webrip") || lower.contains("web-rip") {
        StreamQuality::WebRip
    } else if lower.contains("web-dl") || lower.contains("webdl") {
        StreamQuality::WebDl
    } else if lower.contains("remux") {
        StreamQuality::BluRayRemux
    } else if lower.contains("bluray") || lower.contains("blu-ray") {
        StreamQuality::BluRay
    } else if lower.contains("hdtv") {
        StreamQuality::Hdtv
    } else if lower.contains("dvdrip") {
        StreamQuality::Dvd
    } else {
        StreamQuality::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::StreamDescriptor;

    fn group_1080p_bluray() -> StreamGroup {
        StreamGroup {
            id: Uuid::nil(),
            name: "1080p · Blu-ray".to_string(),
            filter: StreamFilter {
                match_mode: FilterMatchMode::All,
                rules: vec![
                    StreamRule::Resolution {
                        op: SetOp::In,
                        values: vec![StreamResolution::R1080p],
                    },
                    StreamRule::Quality {
                        op: SetOp::In,
                        values: vec![StreamQuality::BluRay, StreamQuality::BluRayRemux],
                    },
                ],
            },
            priority: 0,
            enabled: true,
            hidden: false,
            created_at: String::new(),
        }
    }

    fn info(filename: &str) -> StreamInfo {
        StreamInfo {
            descriptor: StreamDescriptor::default(),
            filename: Some(filename.to_string()),
            ..Default::default()
        }
    }

    // "UHD BluRay" credits the source disc; the explicit "1080p" token is the
    // real encode resolution. These are real filenames that were falling
    // through to Ungrouped before min_screen_size() was introduced.
    #[test]
    fn uhd_bluray_source_with_explicit_1080p_matches_1080p_group() {
        let group = group_1080p_bluray();
        assert_eq!(
            group.match_outcome(
                &info(
                    "The Martian 2015 Extended Cut UHD BluRay 1080p DD Atmos 5 1 DoVi HDR10 x265-SM737.mkv"
                ),
                None
            ),
            MatchOutcome::Match
        );
        assert_eq!(
            group.match_outcome(
                &info(
                    "The.Housemaid.2025.UHD.BluRay.1080p.DD+Atmos.5.1.DoVi.HDR10+.x265-SM737.mkv"
                ),
                None
            ),
            MatchOutcome::Match
        );
        assert_eq!(
            group.match_outcome(
                &info(
                    "The Housemaid 2025 REPACK UHD BluRay 1080p DD Atmos 5 1 DoVi HDR10 x265-SM737.mkv"
                ),
                None
            ),
            MatchOutcome::Match
        );
    }

    #[test]
    fn genuine_2160p_release_does_not_match_1080p_group() {
        let group = group_1080p_bluray();
        assert_eq!(
            group.match_outcome(
                &info(
                    "The Martian 2015 UHD BluRay 2160p HDR10 DoVi Atmos x265-GROUP.mkv"
                ),
                None
            ),
            MatchOutcome::NoMatch
        );
    }

    fn group_audio_lang(op: SetOp, values: &[&str]) -> StreamGroup {
        StreamGroup {
            id: Uuid::nil(),
            name: "audio".to_string(),
            filter: StreamFilter {
                match_mode: FilterMatchMode::All,
                rules: vec![StreamRule::AudioLanguage {
                    op,
                    values: values
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                }],
            },
            priority: 0,
            enabled: true,
            hidden: false,
            created_at: String::new(),
        }
    }

    fn probe_with_langs(langs: &[&str]) -> crate::api::MediaSourceInfo {
        use crate::api::MediaStream;
        let media_streams = langs
            .iter()
            .map(|l| MediaStream {
                type_: Some(MediaStreamType::Audio),
                language: Some(l.to_string()),
                ..Default::default()
            })
            .collect();
        crate::api::MediaSourceInfo {
            media_streams,
            ..Default::default()
        }
    }

    #[test]
    fn audio_lang_matches_when_listed() {
        let group = group_audio_lang(SetOp::Is, &["rus"]);
        let probe = probe_with_langs(&["rus"]);
        assert_eq!(
            group.match_outcome(&info("a.mkv"), Some(&probe)),
            MatchOutcome::Match
        );
    }

    #[test]
    fn audio_lang_rejects_when_language_differs() {
        let group = group_audio_lang(SetOp::Is, &["rus"]);
        let probe = probe_with_langs(&["eng"]);
        assert_eq!(
            group.match_outcome(&info("a.mkv"), Some(&probe)),
            MatchOutcome::NoMatch
        );
    }

    // An unprobed source passes through (not absorbed by the group, but stays
    // visible via ungrouped). This matches the Size rule's None behavior.
    #[test]
    fn audio_lang_unprobed_is_passthrough() {
        let group = group_audio_lang(SetOp::Is, &["rus"]);
        assert_eq!(
            group.match_outcome(&info("a.mkv"), None),
            MatchOutcome::PassThrough
        );
    }

    #[test]
    fn audio_lang_case_insensitive_and_any_track() {
        let group = group_audio_lang(SetOp::In, &["rus"]);
        assert_eq!(
            group.match_outcome(&info("a.mkv"), Some(&probe_with_langs(&["RUS"]))),
            MatchOutcome::Match
        );
        assert_eq!(
            group.match_outcome(
                &info("a.mkv"),
                Some(&probe_with_langs(&["eng", "rus"]))
            ),
            MatchOutcome::Match
        );
    }

    #[test]
    fn audio_lang_isnot_inverts() {
        let group = group_audio_lang(SetOp::NotIn, &["rus"]);
        assert_eq!(
            group.match_outcome(&info("a.mkv"), Some(&probe_with_langs(&["rus"]))),
            MatchOutcome::NoMatch
        );
        assert_eq!(
            group.match_outcome(&info("a.mkv"), Some(&probe_with_langs(&["eng"]))),
            MatchOutcome::Match
        );
    }

    // Special "no-linguistic-content" codes are treated as no language.
    #[test]
    fn audio_lang_ignores_special_codes() {
        let group = group_audio_lang(SetOp::Is, &["rus"]);
        assert_eq!(
            group.match_outcome(&info("a.mkv"), Some(&probe_with_langs(&["und"]))),
            MatchOutcome::NoMatch
        );
        assert_eq!(
            group.match_outcome(&info("a.mkv"), Some(&probe_with_langs(&["mul"]))),
            MatchOutcome::NoMatch
        );
    }

    #[test]
    fn resolution_group_passes_through_source_without_filename() {
        let group = StreamGroup {
            id: Uuid::nil(),
            name: "1080p".to_string(),
            filter: StreamFilter {
                match_mode: FilterMatchMode::All,
                rules: vec![StreamRule::Resolution {
                    op: SetOp::In,
                    values: vec![StreamResolution::R1080p],
                }],
            },
            priority: 0,
            enabled: true,
            hidden: false,
            created_at: String::new(),
        };
        let si = StreamInfo {
            descriptor: StreamDescriptor::default(),
            filename: None,
            name: None,
            ..Default::default()
        };
        assert_eq!(group.match_outcome(&si, None), MatchOutcome::PassThrough);
    }

    // Mixed filter (All): Resolution Match + AudioLanguage PassThrough → PassThrough.
    // The source is not confidently Russian, so it must NOT be shown as the group
    // representative, but it must also stay visible via ungrouped.
    #[test]
    fn audio_lang_combined_with_resolution_passes_through_in_all_mode() {
        let group = StreamGroup {
            id: Uuid::nil(),
            name: "1080p · rus".to_string(),
            filter: StreamFilter {
                match_mode: FilterMatchMode::All,
                rules: vec![
                    StreamRule::Resolution {
                        op: SetOp::In,
                        values: vec![StreamResolution::R1080p],
                    },
                    StreamRule::AudioLanguage {
                        op: SetOp::Is,
                        values: vec!["rus".to_string()],
                    },
                ],
            },
            priority: 0,
            enabled: true,
            hidden: false,
            created_at: String::new(),
        };
        assert_eq!(
            group.match_outcome(&info("Movie.2024.1080p.BluRay.mkv"), None),
            MatchOutcome::PassThrough
        );
    }

    // Any mode: PassThrough is NOT a success. AudioLanguage PassThrough OR
    // Quality NoMatch → NoMatch (no confident match on any rule).
    #[test]
    fn audio_lang_passthrough_is_not_a_success_in_any_mode() {
        let group = StreamGroup {
            id: Uuid::nil(),
            name: "rus or bluray".to_string(),
            filter: StreamFilter {
                match_mode: FilterMatchMode::Any,
                rules: vec![
                    StreamRule::AudioLanguage {
                        op: SetOp::Is,
                        values: vec!["rus".to_string()],
                    },
                    StreamRule::Quality {
                        op: SetOp::In,
                        values: vec![StreamQuality::BluRay],
                    },
                ],
            },
            priority: 0,
            enabled: true,
            hidden: false,
            created_at: String::new(),
        };
        // WEBRip → Quality NoMatch, AudioLanguage PassThrough (no probe) → NoMatch.
        assert_eq!(
            group.match_outcome(&info("Movie.2024.1080p.WEBRip.mkv"), None),
            MatchOutcome::NoMatch
        );
    }

    fn group_size(op: NumericOp, value: i64) -> StreamGroup {
        StreamGroup {
            id: Uuid::nil(),
            name: "size".to_string(),
            filter: StreamFilter {
                match_mode: FilterMatchMode::All,
                rules: vec![StreamRule::Size { op, value }],
            },
            priority: 0,
            enabled: true,
            hidden: false,
            created_at: String::new(),
        }
    }

    fn info_with_size(filename: &str, size: Option<i64>) -> StreamInfo {
        StreamInfo {
            descriptor: StreamDescriptor::default(),
            filename: Some(filename.to_string()),
            size,
            ..Default::default()
        }
    }

    #[test]
    fn size_gt_matches_large_release() {
        let group = group_size(NumericOp::Gt, 20_000_000_000);
        assert_eq!(
            group.match_outcome(
                &info_with_size("Movie.2024.1080p.BluRay.mkv", Some(35_000_000_000)),
                None
            ),
            MatchOutcome::Match
        );
    }

    #[test]
    fn size_gt_rejects_small_release() {
        let group = group_size(NumericOp::Gt, 20_000_000_000);
        assert_eq!(
            group.match_outcome(
                &info_with_size("Movie.2024.1080p.WEBRip.mkv", Some(2_000_000_000)),
                None
            ),
            MatchOutcome::NoMatch
        );
    }

    // HTTP/debrid/IPTV sources almost never report a size. Unknown size passes
    // through (PassThrough) so they stay visible via the ungrouped list.
    #[test]
    fn size_unknown_is_passthrough() {
        let group = group_size(NumericOp::Gt, 20_000_000_000);
        assert_eq!(
            group.match_outcome(&info_with_size("Movie.2024.1080p.mkv", None), None),
            MatchOutcome::PassThrough
        );
    }

    #[test]
    fn size_ops_eq_not_eq_lt() {
        let v = 5_000_000_000;
        assert_eq!(
            group_size(NumericOp::Eq, v)
                .match_outcome(&info_with_size("a.mkv", Some(v)), None),
            MatchOutcome::Match
        );
        assert_eq!(
            group_size(NumericOp::Eq, v)
                .match_outcome(&info_with_size("a.mkv", Some(v + 1)), None),
            MatchOutcome::NoMatch
        );
        assert_eq!(
            group_size(NumericOp::NotEq, v)
                .match_outcome(&info_with_size("a.mkv", Some(v + 1)), None),
            MatchOutcome::Match
        );
        assert_eq!(
            group_size(NumericOp::Lt, v)
                .match_outcome(&info_with_size("a.mkv", Some(v - 1)), None),
            MatchOutcome::Match
        );
    }

    #[test]
    fn size_combined_with_resolution_all() {
        let group = StreamGroup {
            id: Uuid::nil(),
            name: "1080p · big".to_string(),
            filter: StreamFilter {
                match_mode: FilterMatchMode::All,
                rules: vec![
                    StreamRule::Resolution {
                        op: SetOp::In,
                        values: vec![StreamResolution::R1080p],
                    },
                    StreamRule::Size {
                        op: NumericOp::Gt,
                        value: 20_000_000_000,
                    },
                ],
            },
            priority: 0,
            enabled: true,
            hidden: false,
            created_at: String::new(),
        };
        assert_eq!(
            group.match_outcome(
                &info_with_size("Movie.2024.1080p.BluRay.mkv", Some(35_000_000_000)),
                None
            ),
            MatchOutcome::Match
        );
        assert_eq!(
            group.match_outcome(
                &info_with_size("Movie.2024.1080p.WEBRip.mkv", Some(1_000_000_000)),
                None
            ),
            MatchOutcome::NoMatch
        );
        assert_eq!(
            group.match_outcome(
                &info_with_size("Movie.2024.720p.BluRay.mkv", Some(35_000_000_000)),
                None
            ),
            MatchOutcome::NoMatch
        );
    }

    #[test]
    fn bitrate_uses_probe_else_size_over_runtime() {
        const GIB: i64 = 1024 * 1024 * 1024;
        let mut group = group_size(NumericOp::Lt, 0);
        group
            .filter
            .rules = vec![StreamRule::Bitrate {
            op: NumericOp::Lt,
            value: 8_000_000,
        }];
        let file = |size| info_with_size("Movie.1080p.WEB-DL.mkv", size);
        let probe = crate::api::MediaSourceInfo {
            bitrate: Some(20_000_000),
            ..Default::default()
        };
        // 150 min: 2 GiB ≈ 1.9 Mbps, 10 GiB ≈ 9.5 Mbps; 22 min: 2 GiB ≈ 13 Mbps.
        let cases = [
            (Some(2 * GIB), None, Some(9000), MatchOutcome::Match),
            (Some(10 * GIB), None, Some(9000), MatchOutcome::NoMatch),
            (Some(2 * GIB), None, Some(1320), MatchOutcome::NoMatch),
            (
                Some(2 * GIB),
                Some(&probe),
                Some(9000),
                MatchOutcome::NoMatch,
            ),
            (None, None, Some(9000), MatchOutcome::PassThrough),
            (Some(2 * GIB), None, None, MatchOutcome::PassThrough),
        ];
        for (size, probe, runtime, want) in cases {
            assert_eq!(group.match_stream(&file(size), probe, runtime), want);
        }
    }

    // --- detect_stream_quality / quality_weight (pre-probe ordering) ---

    #[test]
    fn detect_stream_quality_parses_resolution_and_quality_from_filename() {
        let (resolution, quality, _codec) = detect_stream_quality(&info(
            "The Martian 2015 UHD BluRay 2160p HDR10 DoVi Atmos x265-GROUP.mkv",
        ))
        .expect("expected a parseable filename");
        assert_eq!(resolution, StreamResolution::R2160p);
        assert_eq!(quality, StreamQuality::BluRay);

        let (resolution, quality, _codec) =
            detect_stream_quality(&info("Movie.2024.1080p.WEBRip.mkv"))
                .expect("expected a parseable filename");
        assert_eq!(resolution, StreamResolution::R1080p);
        assert_eq!(quality, StreamQuality::WebRip);
    }

    #[test]
    fn detect_stream_quality_returns_none_without_filename_or_name() {
        let info = StreamInfo {
            descriptor: StreamDescriptor::default(),
            ..Default::default()
        };
        assert!(detect_stream_quality(&info).is_none());
    }

    fn stream_media(filename: Option<&str>) -> Media {
        Media {
            stream_info: filename.map(|f| info(f)),
            ..Default::default()
        }
    }

    #[test]
    fn quality_weight_ranks_2160p_above_1080p_above_720p() {
        let m2160 = stream_media(Some(
            "The Martian 2015 UHD BluRay 2160p HDR10 DoVi Atmos x265-GROUP.mkv",
        ));
        let m1080 = stream_media(Some("Movie.2024.1080p.WEBRip.mkv"));
        let m720 = stream_media(Some("Movie.2024.720p.WEBRip.mkv"));
        assert!(m2160.quality_weight() > m1080.quality_weight());
        assert!(m1080.quality_weight() > m720.quality_weight());
    }

    #[test]
    fn quality_weight_ranks_bluray_above_webdl_above_webrip() {
        let bluray = stream_media(Some(
            "The Martian 2015 Extended Cut UHD BluRay 1080p DD Atmos 5 1 DoVi HDR10 x265-SM737.mkv",
        ));
        let webdl = stream_media(Some("Movie.2024.1080p.WEB-DL.x264-GROUP.mkv"));
        let webrip = stream_media(Some("Movie.2024.1080p.WEBRip.mkv"));
        assert!(bluray.quality_weight() > webdl.quality_weight());
        assert!(webdl.quality_weight() > webrip.quality_weight());
    }

    #[test]
    fn quality_weight_of_unparseable_stream_is_worst_but_does_not_panic() {
        let unknown = stream_media(None);
        let known = stream_media(Some("Movie.2024.1080p.WEBRip.mkv"));
        assert!(known.quality_weight() > unknown.quality_weight());
    }
}
