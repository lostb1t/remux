//! Jellyfin DVR (Timers / SeriesTimers / Recordings) backed by Dispatcharr.
//!
//! Timers and series rules are forwarded live and never persisted; recordings
//! are synced into `media` as `MediaKind::Recording` rows so playback can use
//! the generic item pipeline.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AppContext,
    addons::{dispatcharr, dispatcharr_dvr},
    api, db,
};

#[derive(Debug, Clone)]
pub struct DvrConfig {
    pub addon_id: Uuid,
    pub base_url: String,
    pub api_key: String,
}

pub struct DvrService;

impl DvrService {
    /// First enabled `dispatcharr`-kind addon instance. Jellyfin's DVR API
    /// has no per-provider concept to route by, so with more than one
    /// configured, the first one found wins.
    pub async fn active_config(ctx: &AppContext) -> Option<DvrConfig> {
        let runtimes = ctx
            .addons
            .list();
        let runtime = runtimes
            .iter()
            .find(|r| {
                r.row
                    .enabled
                    && r.row
                        .preset
                        .kind
                        == "dispatcharr"
            })?;
        let config = runtime
            .row
            .preset
            .config
            .expose();
        let base_url = config["base_url"]
            .as_str()
            .filter(|s| !s.is_empty())?
            .trim_end_matches('/')
            .to_string();
        let api_key = config["api_key"]
            .as_str()
            .filter(|s| !s.is_empty())?
            .to_string();
        Some(DvrConfig {
            addon_id: runtime
                .row
                .id,
            base_url,
            api_key,
        })
    }

    async fn resolve_channel_int(
        cfg: &DvrConfig,
        channel_uuid: Uuid,
    ) -> Result<Option<i64>> {
        let client = dispatcharr::CLIENT.clone();
        let channels =
            dispatcharr::fetch_channels(&client, &cfg.base_url, &cfg.api_key).await?;
        Ok(channels
            .into_iter()
            .find(|ch| channel_uuid_of(cfg.addon_id, ch.id) == channel_uuid)
            .map(|ch| ch.id))
    }

    /// The channel's *authoritative* `tvg_id`, resolved live via
    /// `epg_data_id -> EPGData.tvg_id` — not a cached copy, which
    /// `RefreshDispatcharrLiveTvTask` already documents can go stale.
    async fn resolve_channel_tvg_id(
        cfg: &DvrConfig,
        channel_id: i64,
    ) -> Result<Option<String>> {
        let client = dispatcharr::CLIENT.clone();
        let channels =
            dispatcharr::fetch_channels(&client, &cfg.base_url, &cfg.api_key).await?;
        let Some(epg_data_id) = channels
            .into_iter()
            .find(|ch| ch.id == channel_id)
            .and_then(|ch| ch.epg_data_id)
        else {
            return Ok(None);
        };
        let epg_data =
            dispatcharr::fetch_epg_data(&client, &cfg.base_url, &cfg.api_key).await?;
        Ok(epg_data
            .into_iter()
            .find(|d| d.id == epg_data_id)
            .and_then(|d| d.tvg_id))
    }

    // -- Timers --------------------------------------------------------

    pub async fn list_timers(ctx: &AppContext) -> Result<Vec<TimerInfoDto>> {
        let Some(cfg) = Self::active_config(ctx).await else {
            return Ok(vec![]);
        };
        let client = dispatcharr::CLIENT.clone();
        let recordings =
            dispatcharr_dvr::list_recordings(&client, &cfg.base_url, &cfg.api_key)
                .await?;
        let mut timers: Vec<TimerInfoDto> = recordings
            .iter()
            .filter(|r| {
                matches!(
                    r.status(),
                    dispatcharr_dvr::DispatcharrRecordingStatus::Scheduled
                        | dispatcharr_dvr::DispatcharrRecordingStatus::Recording
                )
            })
            .map(|r| timer_from_recording(cfg.addon_id, r))
            .collect();
        timers.sort_by_key(|t| t.start_date);
        Ok(timers)
    }

    pub async fn get_timer(ctx: &AppContext, id: i64) -> Result<Option<TimerInfoDto>> {
        let Some(cfg) = Self::active_config(ctx).await else {
            return Ok(None);
        };
        let client = dispatcharr::CLIENT.clone();
        match dispatcharr_dvr::get_recording(&client, &cfg.base_url, &cfg.api_key, id)
            .await
        {
            Ok(rec) => Ok(Some(timer_from_recording(cfg.addon_id, &rec))),
            Err(_) => Ok(None),
        }
    }

    pub async fn create_timer(
        ctx: &AppContext,
        req: CreateTimerRequest,
    ) -> Result<TimerInfoDto> {
        let cfg = Self::active_config(ctx)
            .await
            .context("no Dispatcharr DVR configured")?;

        let (channel_uuid, mut start, mut end) = if let Some(pid) = req.program_id {
            let program = db::Media::get_by_id(&ctx.db, &pid)
                .await?
                .context("program not found")?;
            let channel = program
                .parent_id
                .context("program has no parent channel")?;
            let start = req
                .start_date
                .or_else(|| {
                    program
                        .live_start
                        .map(|d| d.and_utc())
                })
                .context("program has no start time")?;
            let end = req
                .end_date
                .or_else(|| {
                    program
                        .live_end
                        .map(|d| d.and_utc())
                })
                .unwrap_or(start + Duration::hours(1));
            (channel, start, end)
        } else {
            (
                req.channel_id
                    .context("ChannelId or ProgramId is required")?,
                req.start_date
                    .context("StartDate is required")?,
                req.end_date
                    .context("EndDate is required")?,
            )
        };

        start -= Duration::seconds(req.pre_padding_seconds as i64);
        end += Duration::seconds(req.post_padding_seconds as i64);

        let channel_id = Self::resolve_channel_int(&cfg, channel_uuid)
            .await?
            .context("channel not found on Dispatcharr")?;
        let rec = dispatcharr_dvr::schedule_recording(
            &dispatcharr::CLIENT,
            &cfg.base_url,
            &cfg.api_key,
            channel_id,
            start,
            end,
        )
        .await?;
        Ok(timer_from_recording(cfg.addon_id, &rec))
    }

    /// Dispatcharr's numeric recording id for a timer id as a client sends it:
    /// the timer's own id, or the UUID of the in-progress `Recording` item a
    /// timer points at through `ProgramInfo` (some clients cancel by
    /// `ProgramInfo.Id`).
    pub async fn resolve_timer_id(ctx: &AppContext, raw: &str) -> Option<i64> {
        if let Ok(id) = raw.parse::<i64>() {
            return Some(id);
        }
        let uuid = raw
            .parse::<Uuid>()
            .ok()?;
        if let Some(media) = db::Media::get_by_id(&ctx.db, &uuid)
            .await
            .ok()
            .flatten()
        {
            return (media.kind == db::MediaKind::Recording)
                .then_some(
                    media
                        .external_ids
                        .dispatcharr_recording_id,
                )
                .flatten();
        }
        // Not synced yet (a client can cancel from the schedule before it ever
        // lists recordings): the id is derived from Dispatcharr's own, so match
        // it against the live list.
        let cfg = Self::active_config(ctx).await?;
        let recordings = dispatcharr_dvr::list_recordings(
            &dispatcharr::CLIENT,
            &cfg.base_url,
            &cfg.api_key,
        )
        .await
        .ok()?;
        recording_id_for_uuid(&recordings, cfg.addon_id, uuid)
    }

    /// Cancels the timer: stops it if it's currently recording (keeping the
    /// partial file), otherwise deletes the not-yet-started scheduled entry.
    pub async fn delete_timer(ctx: &AppContext, id: i64) -> Result<bool> {
        let Some(cfg) = Self::active_config(ctx).await else {
            return Ok(false);
        };
        let client = dispatcharr::CLIENT.clone();
        let Ok(rec) =
            dispatcharr_dvr::get_recording(&client, &cfg.base_url, &cfg.api_key, id)
                .await
        else {
            return Ok(false);
        };
        if rec.status() == dispatcharr_dvr::DispatcharrRecordingStatus::Recording {
            dispatcharr_dvr::stop_recording(&client, &cfg.base_url, &cfg.api_key, id)
                .await?;
        } else {
            dispatcharr_dvr::delete_recording(&client, &cfg.base_url, &cfg.api_key, id)
                .await?;
        }
        Ok(true)
    }

    // -- SeriesTimers ----------------------------------------------------

    pub async fn list_series_timers(
        ctx: &AppContext,
    ) -> Result<Vec<SeriesTimerInfoDto>> {
        let Some(cfg) = Self::active_config(ctx).await else {
            return Ok(vec![]);
        };
        let client = dispatcharr::CLIENT.clone();
        let rules =
            dispatcharr_dvr::list_series_rules(&client, &cfg.base_url, &cfg.api_key)
                .await?;
        Ok(rules
            .iter()
            .map(series_timer_from_rule)
            .collect())
    }

    pub async fn create_series_timer(
        ctx: &AppContext,
        req: CreateSeriesTimerRequest,
    ) -> Result<SeriesTimerInfoDto> {
        let cfg = Self::active_config(ctx)
            .await
            .context("no Dispatcharr DVR configured")?;

        let (channel_uuid, name) = if let Some(pid) = req.program_id {
            let program = db::Media::get_by_id(&ctx.db, &pid)
                .await?
                .context("program not found")?;
            let channel = program
                .parent_id
                .context("program has no parent channel")?;
            let name = req
                .name
                .unwrap_or(program.title);
            (channel, name)
        } else {
            (
                req.channel_id
                    .context("ChannelId or ProgramId is required")?,
                req.name
                    .context(
                        "Name is required when ChannelId is given without ProgramId",
                    )?,
            )
        };

        let channel_id = Self::resolve_channel_int(&cfg, channel_uuid)
            .await?
            .context("channel not found on Dispatcharr")?;
        let tvg_id = Self::resolve_channel_tvg_id(&cfg, channel_id)
            .await?
            .context("channel has no EPG mapping on Dispatcharr")?;

        let request = dispatcharr_dvr::SeriesRuleRequest {
            tvg_id: Some(tvg_id),
            mode: if req.record_new_only { "new" } else { "all" }.to_string(),
            title: Some(name),
            epg_source_id: None,
        };
        let rules = dispatcharr_dvr::create_series_rule(
            &dispatcharr::CLIENT,
            &cfg.base_url,
            &cfg.api_key,
            &request,
        )
        .await?;
        let created = rules
            .iter()
            .find(|r| r.tvg_id == request.tvg_id && r.title == request.title)
            .or_else(|| rules.last())
            .context("Dispatcharr returned no rules after create")?;
        Ok(series_timer_from_rule(created))
    }

    pub async fn delete_series_timer(ctx: &AppContext, id: &str) -> Result<bool> {
        let Some(cfg) = Self::active_config(ctx).await else {
            return Ok(false);
        };
        let Some((tvg_id, title, epg_source_id)) = parse_series_timer_id(id) else {
            return Ok(false);
        };
        dispatcharr_dvr::delete_series_rule(
            &dispatcharr::CLIENT,
            &cfg.base_url,
            &cfg.api_key,
            tvg_id.as_deref(),
            title.as_deref(),
            epg_source_id,
        )
        .await?;
        Ok(true)
    }

    // -- Recordings ------------------------------------------------------
    //
    // `RefreshDispatcharrLiveTvTask` runs every few hours by default, so
    // every read here re-syncs live first via `sync_recordings` — a
    // Dispatcharr `list_recordings()` call is cheap for a homelab-sized DVR.

    /// Fetches current recordings from Dispatcharr and upserts/prunes the
    /// synced `Recording` rows (+ their `Stream` children) for one addon.
    /// Shared by the periodic task and every on-access read below — the
    /// task calls this once per configured Dispatcharr addon; on-access
    /// reads call it once for `active_config()`'s single addon.
    pub async fn sync_recordings(ctx: &AppContext, cfg: &DvrConfig) -> Result<usize> {
        let client = dispatcharr::CLIENT.clone();
        let source_id = cfg
            .addon_id
            .simple()
            .to_string();

        let mut recordings =
            dispatcharr_dvr::list_recordings(&client, &cfg.base_url, &cfg.api_key)
                .await?;
        // Not-yet-started recordings have no file on Dispatcharr's side yet,
        // so syncing them as a playable `Recording` row would make them
        // appear watchable when they aren't — they already surface
        // correctly as Timers.
        recordings.retain(|r| {
            !matches!(
                r.status(),
                dispatcharr_dvr::DispatcharrRecordingStatus::Scheduled
            )
        });

        let recording_rows: Vec<db::Media> = recordings
            .iter()
            .map(|rec| dispatcharr::recording_to_media(rec, cfg.addon_id, &source_id))
            .collect();
        db::Media::upsert(&ctx.db, &recording_rows).await?;

        // Parent rows must land first (FK on `parent_id`).
        let now = chrono::Utc::now().naive_utc();
        let stream_rows: Vec<db::Media> = recordings
            .iter()
            .zip(recording_rows.iter())
            .map(|(rec, media)| {
                dispatcharr::recording_stream_to_media(
                    rec,
                    media.id,
                    &dispatcharr::recording_stream_url(
                        ctx.config
                            .port,
                        media.id,
                    ),
                    now,
                )
            })
            .collect();
        db::Media::upsert(&ctx.db, &stream_rows).await?;

        let keep_ids: Vec<Uuid> = recording_rows
            .iter()
            .map(|m| m.id)
            .collect();
        let mut qb = sqlx::QueryBuilder::new(
            "DELETE FROM media WHERE kind = 'recording' AND json_extract(external_ids, '$.iptv_source_id') = ",
        );
        qb.push_bind(&source_id);
        if !keep_ids.is_empty() {
            qb.push(" AND id NOT IN (");
            let mut sep = qb.separated(", ");
            for id in &keep_ids {
                sep.push_bind(id);
            }
            qb.push(")");
        }
        qb.build()
            .execute(&ctx.db)
            .await?;

        Ok(recordings.len())
    }

    pub async fn list_recordings(ctx: &AppContext) -> Result<Vec<api::BaseItemDto>> {
        if let Some(cfg) = Self::active_config(ctx).await {
            if let Err(e) = Self::sync_recordings(ctx, &cfg).await {
                tracing::warn!(error = %e, "failed to refresh recordings from Dispatcharr, serving last-synced state");
            }
        }
        let mut result = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Recording]),
                ..Default::default()
            },
        )
        .await?;
        db::Media::attach_streams(&ctx.db, &mut result.records).await?;
        let mut items: Vec<api::BaseItemDto> = result
            .records
            .into_iter()
            .map(|m| api::db_media_to_item(m, false))
            .collect();
        newest_first(&mut items);
        Ok(items)
    }

    pub async fn get_recording(
        ctx: &AppContext,
        id: Uuid,
    ) -> Result<Option<api::BaseItemDto>> {
        if db::Media::get_by_id(&ctx.db, &id)
            .await?
            .is_none()
        {
            // Not synced yet (e.g. it just finished) — try once before
            // giving up, rather than always paying the round trip.
            if let Some(cfg) = Self::active_config(ctx).await {
                let _ = Self::sync_recordings(ctx, &cfg).await;
            }
        }
        let Some(mut media) = db::Media::get_by_id(&ctx.db, &id).await? else {
            return Ok(None);
        };
        if media.kind != db::MediaKind::Recording {
            return Ok(None);
        }
        media.sources = Some(
            media
                .streams(&ctx.db)
                .await?,
        );
        Ok(Some(api::db_media_to_item(media, false)))
    }

    pub async fn delete_recording(ctx: &AppContext, id: Uuid) -> Result<bool> {
        let Some(media) = db::Media::get_by_id(&ctx.db, &id).await? else {
            return Ok(false);
        };
        if media.kind != db::MediaKind::Recording {
            return Ok(false);
        }
        if let (Some(dispatcharr_id), Some(cfg)) = (
            media
                .external_ids
                .dispatcharr_recording_id,
            Self::active_config(ctx).await,
        ) {
            dispatcharr_dvr::delete_recording(
                &dispatcharr::CLIENT,
                &cfg.base_url,
                &cfg.api_key,
                dispatcharr_id,
            )
            .await?;
        }
        db::Media::delete(&ctx.db, &id).await?;
        Ok(true)
    }

    /// Resolves a synced recording to what the stream proxy needs: the
    /// Dispatcharr config to reach it with, plus its *current*
    /// `DispatcharrRecording` (fetched live — `file_url`/status change as a
    /// recording moves from in-progress to finished, so the DB row, synced
    /// only periodically, can't be trusted for this).
    pub async fn recording_playback_target(
        ctx: &AppContext,
        id: Uuid,
    ) -> Result<Option<(DvrConfig, dispatcharr_dvr::DispatcharrRecording)>> {
        let Some(media) = db::Media::get_by_id(&ctx.db, &id).await? else {
            return Ok(None);
        };
        let Some(dispatcharr_id) = media
            .external_ids
            .dispatcharr_recording_id
        else {
            return Ok(None);
        };
        let Some(cfg) = Self::active_config(ctx).await else {
            return Ok(None);
        };
        let rec = dispatcharr_dvr::get_recording(
            &dispatcharr::CLIENT,
            &cfg.base_url,
            &cfg.api_key,
            dispatcharr_id,
        )
        .await?;
        Ok(Some((cfg, rec)))
    }
}

/// Newest recording first, the order clients expect for "recent recordings"
/// (Moonfin shows the list as given). Done here because the DB layer has no
/// sort key for a recording's start: `ItemSortBy::StartDate` is not handled
/// there, so asking it to sort was silently a no-op. Items without a
/// parseable start go last.
fn newest_first(items: &mut [api::BaseItemDto]) {
    let start = |i: &api::BaseItemDto| {
        i.start_date
            .as_deref()
            .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
    };
    items.sort_by(|a, b| start(b).cmp(&start(a)));
}

/// Dispatcharr's id of the recording whose synced item id is `uuid`.
fn recording_id_for_uuid(
    recordings: &[dispatcharr_dvr::DispatcharrRecording],
    addon_id: Uuid,
    uuid: Uuid,
) -> Option<i64> {
    recordings
        .iter()
        .find(|r| dispatcharr::recording_media_id(addon_id, r.id) == uuid)
        .map(|r| r.id)
}

fn channel_uuid_of(addon_id: Uuid, channel_id: i64) -> Uuid {
    Uuid::new_v5(&addon_id, format!("channel:{channel_id}").as_bytes())
}

fn timer_from_recording(
    addon_id: Uuid,
    rec: &dispatcharr_dvr::DispatcharrRecording,
) -> TimerInfoDto {
    TimerInfoDto {
        id: rec
            .id
            .to_string(),
        type_: "Timer".to_string(),
        server_id: crate::common::server_id(),
        channel_id: Some(channel_uuid_of(addon_id, rec.channel)),
        program_id: None,
        name: rec
            .program_title()
            .unwrap_or("Recording")
            .to_string(),
        overview: rec
            .program_description()
            .map(str::to_owned),
        start_date: rec.start_time,
        end_date: rec.end_time,
        service_name: "dispatcharr".to_string(),
        priority: 0,
        pre_padding_seconds: 0,
        post_padding_seconds: 0,
        is_pre_padding_required: false,
        is_post_padding_required: false,
        status: RecordingStatus::from(rec.status()),
        series_timer_id: None,
        program_info: (rec.status()
            == dispatcharr_dvr::DispatcharrRecordingStatus::Recording)
            .then(|| {
                api::db_media_to_item(
                    dispatcharr::recording_to_media(
                        rec,
                        addon_id,
                        &addon_id
                            .simple()
                            .to_string(),
                    ),
                    false,
                )
            }),
    }
}

fn series_timer_id(rule: &dispatcharr_dvr::DispatcharrSeriesRule) -> String {
    let raw = format!(
        "{}|{}|{}",
        rule.tvg_id
            .as_deref()
            .unwrap_or(""),
        rule.title
            .as_deref()
            .unwrap_or(""),
        rule.epg_source_id
            .map(|v| v.to_string())
            .unwrap_or_default(),
    );
    urlencoding::encode(&raw).into_owned()
}

fn parse_series_timer_id(
    id: &str,
) -> Option<(Option<String>, Option<String>, Option<i64>)> {
    let decoded = urlencoding::decode(id).ok()?;
    // The title is free text and may itself contain `|`, so it is whatever sits
    // between the first separator (after `tvg_id`) and the last (before the
    // numeric `epg_source_id`).
    let (tvg_id, rest) = decoded
        .split_once('|')
        .unwrap_or((&decoded, ""));
    let (title, epg_source_id) = rest
        .rsplit_once('|')
        .unwrap_or((rest, ""));
    let tvg_id = Some(tvg_id)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let title = Some(title)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let epg_source_id = Some(epg_source_id)
        .filter(|s| !s.is_empty())
        .and_then(|s| {
            s.parse()
                .ok()
        });
    Some((tvg_id, title, epg_source_id))
}

fn series_timer_from_rule(
    rule: &dispatcharr_dvr::DispatcharrSeriesRule,
) -> SeriesTimerInfoDto {
    SeriesTimerInfoDto {
        id: series_timer_id(rule),
        type_: "SeriesTimer".to_string(),
        server_id: crate::common::server_id(),
        name: rule
            .title
            .clone()
            .unwrap_or_else(|| "All programs".to_string()),
        overview: rule
            .description
            .clone(),
        service_name: "dispatcharr".to_string(),
        priority: 0,
        record_any_time: true,
        record_any_channel: rule
            .tvg_id
            .is_none(),
        record_new_only: rule.mode == "new",
        skip_episodes_in_library: false,
    }
}

// ---------------------------------------------------------------------------
// Wire DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum RecordingStatus {
    New,
    InProgress,
    Completed,
    Cancelled,
    Error,
}

impl From<dispatcharr_dvr::DispatcharrRecordingStatus> for RecordingStatus {
    fn from(s: dispatcharr_dvr::DispatcharrRecordingStatus) -> Self {
        use dispatcharr_dvr::DispatcharrRecordingStatus as S;
        match s {
            S::Scheduled => RecordingStatus::New,
            S::Recording => RecordingStatus::InProgress,
            S::Completed => RecordingStatus::Completed,
            S::Stopped | S::Interrupted => RecordingStatus::Cancelled,
            S::Failed => RecordingStatus::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TimerInfoDto {
    pub id: String,
    #[serde(rename = "Type")]
    pub type_: String,
    pub server_id: String,
    pub channel_id: Option<Uuid>,
    pub program_id: Option<Uuid>,
    pub name: String,
    pub overview: Option<String>,
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub service_name: String,
    pub priority: i32,
    pub pre_padding_seconds: i32,
    pub post_padding_seconds: i32,
    pub is_pre_padding_required: bool,
    pub is_post_padding_required: bool,
    pub status: RecordingStatus,
    pub series_timer_id: Option<String>,
    /// Only while the recording is in progress: the playable `Recording` item.
    /// A client that lists timers as "scheduled" (Moonfin ignores `Status`)
    /// can then open it, since a timer's own id isn't an item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program_info: Option<api::BaseItemDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SeriesTimerInfoDto {
    pub id: String,
    #[serde(rename = "Type")]
    pub type_: String,
    pub server_id: String,
    pub name: String,
    pub overview: Option<String>,
    pub service_name: String,
    pub priority: i32,
    pub record_any_time: bool,
    pub record_any_channel: bool,
    pub record_new_only: bool,
    pub skip_episodes_in_library: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateTimerRequest {
    pub program_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub pre_padding_seconds: i32,
    #[serde(default)]
    pub post_padding_seconds: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateSeriesTimerRequest {
    pub program_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub name: Option<String>,
    #[serde(default)]
    pub record_new_only: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::dispatcharr_dvr::{
        DispatcharrRecording, DispatcharrRecordingStatus as Status,
        DispatcharrSeriesRule,
    };
    use serde_json::{Value, json};

    const ADDON: Uuid = Uuid::from_u128(0xd15b);

    fn recording(id: i64, custom_properties: Value) -> DispatcharrRecording {
        serde_json::from_value(json!({
            "id": id,
            "channel": 42,
            "start_time": "2026-09-15T20:00:00Z",
            "end_time": "2026-09-15T21:00:00Z",
            "custom_properties": custom_properties,
        }))
        .unwrap()
    }

    fn rule(
        tvg: Option<&str>,
        title: Option<&str>,
        epg: Option<i64>,
    ) -> DispatcharrSeriesRule {
        DispatcharrSeriesRule {
            tvg_id: tvg.map(str::to_owned),
            mode: "all".into(),
            title: title.map(str::to_owned),
            description: None,
            epg_source_id: epg,
        }
    }

    // -- series timer ids ---------------------------------------------

    #[test]
    fn series_timer_id_is_the_percent_encoded_composite() {
        let id =
            series_timer_id(&rule(Some("BBC1.uk"), Some("Match of the Day"), Some(3)));
        assert_eq!(id, "BBC1.uk%7CMatch%20of%20the%20Day%7C3");
    }

    #[test]
    fn series_timer_id_round_trips() {
        let cases = [
            (Some("BBC1.uk"), Some("Match of the Day"), Some(3)),
            (Some("BBC1.uk"), Some("Match of the Day"), None),
            (None, Some("Any channel"), None),
            (Some("BBC1.uk"), None, None),
            (None, None, None),
            (Some("a b"), Some("Q&A: 50% off"), Some(12)),
        ];
        for (tvg, title, epg) in cases {
            let id = series_timer_id(&rule(tvg, title, epg));
            let want = (tvg.map(str::to_owned), title.map(str::to_owned), epg);
            assert_eq!(parse_series_timer_id(&id), Some(want), "{id}");
        }
    }

    #[test]
    fn series_timer_id_round_trips_a_title_containing_the_separator() {
        let id = series_timer_id(&rule(Some("BBC1.uk"), Some("Cats | Dogs"), Some(3)));
        let want = (
            Some("BBC1.uk".to_owned()),
            Some("Cats | Dogs".to_owned()),
            Some(3),
        );
        assert_eq!(parse_series_timer_id(&id), Some(want), "{id}");
    }

    #[test]
    fn parse_series_timer_id_accepts_an_already_decoded_id() {
        // axum percent-decodes path params before the handler sees them.
        assert_eq!(
            parse_series_timer_id("BBC1.uk|Match of the Day|3"),
            Some((
                Some("BBC1.uk".into()),
                Some("Match of the Day".into()),
                Some(3)
            ))
        );
    }

    #[test]
    fn parse_series_timer_id_ignores_a_non_numeric_source() {
        assert_eq!(
            parse_series_timer_id("BBC1.uk|MOTD|abc"),
            Some((Some("BBC1.uk".into()), Some("MOTD".into()), None))
        );
    }

    #[test]
    fn parse_series_timer_id_rejects_invalid_utf8() {
        assert_eq!(parse_series_timer_id("%FF%FE"), None);
    }

    // -- conversions --------------------------------------------------

    #[test]
    fn channel_uuid_matches_the_synced_channel_row() {
        let ch: dispatcharr::DispatcharrChannel =
            serde_json::from_value(json!({ "id": 42, "uuid": "u", "name": "BBC One" }))
                .unwrap();
        assert_eq!(
            channel_uuid_of(ADDON, 42),
            dispatcharr::channel_to_media(&ch, ADDON, "s").id
        );
    }

    #[test]
    fn timer_from_recording_maps_fields() {
        let rec = recording(
            7,
            json!({ "status": "recording",
                    "program": { "title": "MOTD", "description": "Highlights" } }),
        );
        let t = timer_from_recording(ADDON, &rec);
        assert_eq!(t.id, "7");
        assert_eq!(t.type_, "Timer");
        assert_eq!(t.name, "MOTD");
        assert_eq!(
            t.overview
                .as_deref(),
            Some("Highlights")
        );
        assert_eq!(t.channel_id, Some(channel_uuid_of(ADDON, 42)));
        assert_eq!(t.start_date, rec.start_time);
        assert_eq!(t.end_date, rec.end_time);
        assert_eq!(t.service_name, "dispatcharr");
        assert_eq!(t.status, RecordingStatus::InProgress);
        assert_eq!(t.server_id, crate::common::server_id());
        assert_eq!(t.series_timer_id, None);
    }

    #[test]
    fn only_a_recording_in_progress_carries_its_item_as_program_info() {
        let running = recording(7, json!({ "status": "recording" }));
        let t = timer_from_recording(ADDON, &running);
        let item = t
            .program_info
            .expect("in-progress timer points at its recording");
        assert_eq!(
            item.id,
            Uuid::new_v5(&ADDON, b"recording:7"),
            "same id the synced Recording row gets"
        );
        for status in [json!({}), json!({ "status": "completed" })] {
            assert!(
                timer_from_recording(ADDON, &recording(7, status))
                    .program_info
                    .is_none()
            );
        }
        let v =
            serde_json::to_value(timer_from_recording(ADDON, &recording(7, json!({}))))
                .unwrap();
        assert!(
            v.get("ProgramInfo")
                .is_none(),
            "omitted when absent: {v}"
        );
    }

    #[test]
    fn recordings_are_listed_newest_first() {
        let item = |start: Option<&str>| api::BaseItemDto {
            start_date: start.map(str::to_owned),
            ..Default::default()
        };
        let mut items = vec![
            item(Some("2026-09-04T17:33:24.674911+00:00")),
            item(None),
            item(Some("2026-09-21T20:20:54.518446+00:00")),
            item(Some("2026-09-12T22:00:00+00:00")),
        ];
        newest_first(&mut items);
        let order: Vec<_> = items
            .iter()
            .map(|i| {
                i.start_date
                    .clone()
            })
            .collect();
        assert_eq!(
            order,
            vec![
                Some("2026-09-21T20:20:54.518446+00:00".to_string()),
                Some("2026-09-12T22:00:00+00:00".to_string()),
                Some("2026-09-04T17:33:24.674911+00:00".to_string()),
                None,
            ]
        );
    }

    #[tokio::test]
    async fn a_timer_id_resolves_from_its_number_or_its_recordings_uuid() {
        use crate::integration_test::new_test_server;

        let (_server, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let rec = recording(7, json!({ "status": "recording" }));
        let channel: dispatcharr::DispatcharrChannel =
            serde_json::from_value(json!({ "id": 42, "uuid": "u", "name": "BBC" }))
                .unwrap();
        db::Media::upsert(
            &ctx.db,
            &[dispatcharr::channel_to_media(&channel, ADDON, "src")],
        )
        .await
        .unwrap();
        let media = dispatcharr::recording_to_media(&rec, ADDON, "src");
        db::Media::upsert(&ctx.db, &[media.clone()])
            .await
            .unwrap();

        assert_eq!(DvrService::resolve_timer_id(ctx, "7").await, Some(7));
        assert_eq!(
            DvrService::resolve_timer_id(
                ctx,
                &media
                    .id
                    .to_string()
            )
            .await,
            Some(7)
        );
        assert_eq!(
            DvrService::resolve_timer_id(ctx, &Uuid::new_v4().to_string()).await,
            None
        );
        assert_eq!(DvrService::resolve_timer_id(ctx, "nope").await, None);
    }

    #[test]
    fn a_recording_is_found_by_its_derived_item_id_before_it_is_synced() {
        let recs = [
            recording(3, json!({})),
            recording(7, json!({ "status": "recording" })),
        ];
        let wanted = dispatcharr::recording_media_id(ADDON, 7);
        assert_eq!(recording_id_for_uuid(&recs, ADDON, wanted), Some(7));
        // The id depends on the addon, so another addon's recording never matches.
        let other = dispatcharr::recording_media_id(Uuid::from_u128(1), 7);
        assert_eq!(recording_id_for_uuid(&recs, ADDON, other), None);
        assert_eq!(recording_id_for_uuid(&[], ADDON, wanted), None);
    }

    #[test]
    fn timer_from_recording_names_unlabelled_recordings() {
        let t = timer_from_recording(ADDON, &recording(1, json!({})));
        assert_eq!(t.name, "Recording");
        assert_eq!(t.overview, None);
        assert_eq!(t.status, RecordingStatus::New);
    }

    #[test]
    fn timer_serialises_with_jellyfin_field_names() {
        let t = timer_from_recording(
            ADDON,
            &recording(1, json!({ "status": "completed" })),
        );
        let v = serde_json::to_value(t).unwrap();
        for key in [
            "Id",
            "Type",
            "ServerId",
            "ChannelId",
            "Name",
            "StartDate",
            "EndDate",
            "ServiceName",
            "PrePaddingSeconds",
            "IsPrePaddingRequired",
            "Status",
        ] {
            assert!(
                v.get(key)
                    .is_some(),
                "missing {key} in {v}"
            );
        }
        assert_eq!(v["Status"], "Completed");
    }

    #[test]
    fn series_timer_from_rule_maps_fields() {
        let mut r = rule(Some("BBC1.uk"), Some("MOTD"), Some(3));
        r.mode = "new".into();
        r.description = Some("desc".into());
        let s = series_timer_from_rule(&r);
        assert_eq!(s.id, series_timer_id(&r));
        assert_eq!(s.type_, "SeriesTimer");
        assert_eq!(s.name, "MOTD");
        assert_eq!(
            s.overview
                .as_deref(),
            Some("desc")
        );
        assert!(s.record_new_only);
        assert!(!s.record_any_channel);
        assert!(s.record_any_time);
    }

    #[test]
    fn series_timer_without_tvg_id_records_any_channel() {
        let s = series_timer_from_rule(&rule(None, None, None));
        assert_eq!(s.name, "All programs");
        assert!(s.record_any_channel);
        assert!(!s.record_new_only, "mode \"all\" is not new-only");
    }

    #[test]
    fn recording_status_maps_to_jellyfin_states() {
        for (from, want) in [
            (Status::Scheduled, RecordingStatus::New),
            (Status::Recording, RecordingStatus::InProgress),
            (Status::Completed, RecordingStatus::Completed),
            (Status::Stopped, RecordingStatus::Cancelled),
            (Status::Interrupted, RecordingStatus::Cancelled),
            (Status::Failed, RecordingStatus::Error),
        ] {
            assert_eq!(RecordingStatus::from(from), want, "{from:?}");
        }
    }

    #[test]
    fn recording_status_serialises_pascal_case() {
        let s = |st| serde_json::to_value(st).unwrap();
        assert_eq!(s(RecordingStatus::InProgress), "InProgress");
        assert_eq!(s(RecordingStatus::New), "New");
        assert_eq!(s(RecordingStatus::Cancelled), "Cancelled");
    }

    #[test]
    fn create_requests_deserialise_jellyfin_bodies() {
        let t: CreateTimerRequest = serde_json::from_value(json!({
            "ChannelId": "00000000-0000-0000-0000-000000000001",
            "StartDate": "2026-10-20T03:00:00Z",
            "EndDate": "2026-10-20T03:10:00Z",
        }))
        .unwrap();
        assert_eq!(t.channel_id, Some(Uuid::from_u128(1)));
        assert_eq!(t.program_id, None);
        assert_eq!((t.pre_padding_seconds, t.post_padding_seconds), (0, 0));

        let padded: CreateTimerRequest = serde_json::from_value(json!({
            "ProgramId": "00000000-0000-0000-0000-000000000002",
            "PrePaddingSeconds": 60, "PostPaddingSeconds": 300,
        }))
        .unwrap();
        assert_eq!(
            (padded.pre_padding_seconds, padded.post_padding_seconds),
            (60, 300)
        );

        let s: CreateSeriesTimerRequest =
            serde_json::from_value(json!({ "Name": "MOTD", "RecordNewOnly": true }))
                .unwrap();
        assert!(s.record_new_only);
        assert_eq!(
            s.name
                .as_deref(),
            Some("MOTD")
        );
        let default: CreateSeriesTimerRequest =
            serde_json::from_value(json!({})).unwrap();
        assert!(!default.record_new_only);
    }

    // -- sync_recordings ---------------------------------------------

    fn rec_json(id: i64, status: Option<&str>) -> Value {
        let props = match status {
            Some(s) => {
                json!({ "status": s, "program": { "title": format!("Show {id}") } })
            }
            None => json!({}),
        };
        json!({
            "id": id, "channel": 42,
            "start_time": "2026-09-15T20:00:00Z",
            "end_time": "2026-09-15T21:00:00Z",
            "custom_properties": props,
        })
    }

    async fn seed_channel(ctx: &AppContext, addon: Uuid, source: &str) {
        let ch: dispatcharr::DispatcharrChannel =
            serde_json::from_value(json!({ "id": 42, "uuid": "u", "name": "BBC One" }))
                .unwrap();
        db::Media::upsert(
            &ctx.db,
            &vec![dispatcharr::channel_to_media(&ch, addon, source)],
        )
        .await
        .unwrap();
    }

    async fn recording_ids(ctx: &AppContext) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Recording]),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .records
        .into_iter()
        .map(|m| m.id)
        .collect();
        ids.sort();
        ids
    }

    async fn stream_count(ctx: &AppContext) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE kind = 'stream'")
            .fetch_one(&ctx.db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn sync_recordings_skips_scheduled_and_prunes_only_its_own_addon() {
        use crate::integration_test::new_test_server;

        let (_server, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let other_addon = Uuid::from_u128(0x0dd);
        seed_channel(
            ctx,
            ADDON,
            &ADDON
                .simple()
                .to_string(),
        )
        .await;
        seed_channel(
            ctx,
            other_addon,
            &other_addon
                .simple()
                .to_string(),
        )
        .await;

        // A recording that belongs to a different Dispatcharr addon.
        let foreign = dispatcharr::recording_to_media(
            &recording(99, json!({ "status": "completed" })),
            other_addon,
            &other_addon
                .simple()
                .to_string(),
        );
        db::Media::upsert(&ctx.db, &vec![foreign.clone()])
            .await
            .unwrap();

        let server = httpmock::MockServer::start();
        let mut list = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/channels/recordings/")
                .header("X-API-Key", "k");
            then.status(200)
                .json_body(json!([
                    rec_json(1, None),
                    rec_json(2, Some("recording")),
                    rec_json(3, Some("completed")),
                    rec_json(4, Some("failed")),
                ]));
        });
        let cfg = DvrConfig {
            addon_id: ADDON,
            base_url: server.base_url(),
            api_key: "k".into(),
        };
        let rid = |n: i64| Uuid::new_v5(&ADDON, format!("recording:{n}").as_bytes());

        // Scheduled (id 1) has no file yet, so it is not synced as playable.
        assert_eq!(
            DvrService::sync_recordings(ctx, &cfg)
                .await
                .unwrap(),
            3
        );
        let mut want = vec![rid(2), rid(3), rid(4), foreign.id];
        want.sort();
        assert_eq!(recording_ids(ctx).await, want);
        assert_eq!(
            stream_count(ctx).await,
            3,
            "one playable child per recording"
        );

        // Dispatcharr drops recordings 2 and 4: they are pruned, with their
        // stream children, while the other addon's row is left alone.
        list.delete();
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/channels/recordings/");
            then.status(200)
                .json_body(json!([rec_json(3, Some("completed"))]));
        });
        assert_eq!(
            DvrService::sync_recordings(ctx, &cfg)
                .await
                .unwrap(),
            1
        );
        let mut want = vec![rid(3), foreign.id];
        want.sort();
        assert_eq!(recording_ids(ctx).await, want);
        assert_eq!(stream_count(ctx).await, 1);
    }

    #[tokio::test]
    async fn sync_recordings_with_none_left_prunes_everything_for_the_addon() {
        use crate::integration_test::new_test_server;

        let (_server, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        seed_channel(
            ctx,
            ADDON,
            &ADDON
                .simple()
                .to_string(),
        )
        .await;

        let server = httpmock::MockServer::start();
        let mut list = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/channels/recordings/");
            then.status(200)
                .json_body(json!([rec_json(3, Some("completed"))]));
        });
        let cfg = DvrConfig {
            addon_id: ADDON,
            base_url: server.base_url(),
            api_key: "k".into(),
        };
        assert_eq!(
            DvrService::sync_recordings(ctx, &cfg)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            recording_ids(ctx)
                .await
                .len(),
            1
        );

        list.delete();
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/channels/recordings/");
            then.status(200)
                .json_body(json!([]));
        });
        assert_eq!(
            DvrService::sync_recordings(ctx, &cfg)
                .await
                .unwrap(),
            0
        );
        assert!(
            recording_ids(ctx)
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn sync_recordings_reports_a_dispatcharr_failure() {
        use crate::integration_test::new_test_server;

        let (_server, guard) = new_test_server()
            .await
            .unwrap();
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.any_request();
            then.status(500);
        });
        let cfg = DvrConfig {
            addon_id: ADDON,
            base_url: server.base_url(),
            api_key: "k".into(),
        };
        assert!(
            DvrService::sync_recordings(&guard.0, &cfg)
                .await
                .is_err()
        );
    }
}
