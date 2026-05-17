use anyhow::Result;
use chrono::Utc;
use remux_sdks::remux::{
    FilterMatchMode, SetOp, StreamCodec, StreamFilter, StreamResolution, StreamRule,
    StreamSource,
};
use sqlx::SqlitePool;
use std::collections::HashSet;
use uuid::Uuid;

use crate::db::{Media, Settings, StreamGroupData};
use crate::stream::StreamInfo;

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

impl StreamGroup {
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
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
            name: self.name.clone(),
            filter: self.filter.clone(),
            priority: self.priority,
            hidden: self.hidden,
            created_at: self.created_at.clone(),
        };
        let data_json = serde_json::to_string(&data)?;
        let now = Utc::now().naive_utc();
        sqlx::query(
            "INSERT INTO media (id, kind, title, enabled, stream_group_data, external_ids, created_at, updated_at)
             VALUES (?, 'stream_group', ?, ?, ?, '{}', ?, ?)
             ON CONFLICT (id) DO UPDATE SET
                 title = excluded.title,
                 enabled = excluded.enabled,
                 stream_group_data = excluded.stream_group_data,
                 external_ids = COALESCE(media.external_ids, excluded.external_ids),
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
            let _ = group.save(db).await;
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
        let mut groups: Vec<Self> =
            rows.into_iter().filter_map(Self::from_media).collect();
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
            created_at: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        };
        group.save(db).await?;
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
        group.save(db).await?;
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
    pub async fn filter_sources(db: &SqlitePool, sources: Vec<Media>) -> Vec<Media> {
        let groups = match Self::list(db).await {
            Ok(g) => g,
            Err(_) => return sources,
        };
        let enabled: Vec<&StreamGroup> = groups.iter().filter(|g| g.enabled).collect();
        if enabled.is_empty() {
            return sources;
        }

        let show_ungrouped = Settings::get_config(db)
            .await
            .ok()
            .and_then(|c| c.stream_groups_show_ungrouped)
            .unwrap_or(true);

        let mut result: Vec<Media> = vec![];
        let mut matched_ids: HashSet<Uuid> = HashSet::new();

        for group in &enabled {
            let matching: Vec<&Media> = sources
                .iter()
                .filter(|s| {
                    s.stream_info
                        .as_ref()
                        .map_or(false, |info| group.matches(info))
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

    /// Returns true if the given StreamInfo matches this group's filter.
    /// An empty filter (no rules) matches everything.
    pub fn matches(&self, info: &StreamInfo) -> bool {
        let filter = &self.filter;
        if filter.rules.is_empty() {
            return true;
        }

        let raw = match info.filename.as_deref().or(info.name.as_deref()) {
            Some(s) => s,
            None => return false,
        };

        // Stremio/addon sources often use a multi-line name where the first
        // line is a provider prefix and later lines contain the actual filename.
        // Parse each non-empty line with hunch and use the one that yields the
        // richest result (has at least a resolution or source hit).
        let candidates: Vec<&str> = if raw.contains('\n') {
            raw.lines().filter(|l| !l.trim().is_empty()).collect()
        } else {
            vec![raw]
        };

        let best = candidates.iter().map(|s| hunch::hunch(s)).max_by_key(|p| {
            (p.screen_size().is_some() as u8) + (p.source().is_some() as u8)
        });

        let parsed = match best {
            Some(p) => p,
            None => return false,
        };

        let resolution = parsed
            .screen_size()
            .and_then(StreamResolution::from_hunch)
            .unwrap_or(StreamResolution::Unknown);
        let source = canonical_source(&parsed);
        let codec = parsed
            .video_codec()
            .and_then(StreamCodec::from_hunch)
            .unwrap_or(StreamCodec::Unknown);

        let eval = |rule: &StreamRule| match rule {
            StreamRule::Resolution { op, values } => {
                let hit = values.contains(&resolution);
                matches!(op, SetOp::In | SetOp::Is) == hit
            }
            StreamRule::Source { op, values } => {
                let hit = values.contains(&source);
                matches!(op, SetOp::In | SetOp::Is) == hit
            }
            StreamRule::Codec { op, values } => {
                let hit = values.contains(&codec);
                matches!(op, SetOp::In | SetOp::Is) == hit
            }
        };

        match filter.match_mode {
            FilterMatchMode::All => filter.rules.iter().all(eval),
            FilterMatchMode::Any => filter.rules.iter().any(eval),
        }
    }

    fn candidates_for_group(group: &StreamGroup, raw_sources: &[Media]) -> Vec<Media> {
        let mut v: Vec<Media> = raw_sources
            .iter()
            .filter(|s| {
                s.stream_info
                    .as_ref()
                    .map_or(false, |info| group.matches(info))
            })
            .cloned()
            .collect();
        v.sort_by_key(|s| s.idx.unwrap_or(0));
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
        let raw = parent.streams(db).await?;
        let raw = if raw.is_empty() { vec![parent] } else { raw };
        Ok(Self::candidates_for_group(&group, &raw))
    }
}

fn auto_name(filter: &StreamFilter) -> String {
    let parts: Vec<String> = filter
        .rules
        .iter()
        .filter_map(|r| {
            let labels: Vec<&str> = match r {
                StreamRule::Resolution { values, .. } => {
                    values.iter().map(|v| v.label()).collect()
                }
                StreamRule::Source { values, .. } => {
                    values.iter().map(|v| v.label()).collect()
                }
                StreamRule::Codec { values, .. } => {
                    values.iter().map(|v| v.label()).collect()
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

fn canonical_source(parsed: &hunch::HunchResult) -> StreamSource {
    let other = parsed.other();
    let is_remux = other.contains(&"Remux");
    let is_rip = other.contains(&"Rip");
    let Some(source) = parsed.source() else {
        // No source keyword — remux is always Blu-ray regardless
        return if is_remux {
            StreamSource::BluRayRemux
        } else {
            StreamSource::Unknown
        };
    };
    match source {
        "Web" if is_rip => StreamSource::WebRip,
        "Web" => StreamSource::WebDl,
        "Blu-ray" | "Ultra HD Blu-ray" if is_remux => StreamSource::BluRayRemux,
        "Blu-ray" | "Ultra HD Blu-ray" => StreamSource::BluRay,
        "HDTV" => StreamSource::Hdtv,
        "DVD" => StreamSource::Dvd,
        "TV" => StreamSource::Tv,
        _ => StreamSource::Unknown,
    }
}
