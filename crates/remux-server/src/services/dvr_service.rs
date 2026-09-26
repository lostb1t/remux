//! Jellyfin DVR (timers, series timers, recordings) backed by Dispatcharr.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AppContext,
    addons::{dispatcharr, dispatcharr_dvr},
    api, db,
};

/// Ids per batched statement, under SQLite's bind-variable limit.
const SQL_BIND_CHUNK: usize = 500;

/// Serializes `sync_recordings` per addon, so an older snapshot's prune
/// cannot delete a recording a newer, concurrent snapshot just inserted.
static SYNC_RECORDINGS_LOCKS: crate::keyed_lock::KeyedLock<Uuid> =
    crate::keyed_lock::KeyedLock::new();

#[derive(Debug, Clone)]
pub struct DvrConfig {
    pub addon_id: Uuid,
    pub base_url: String,
    pub api_key: String,
}

/// A timer as a client addresses it: a Dispatcharr recording id and the
/// instance it belongs to, serialised as `{addon}:{recording}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerId {
    pub addon_id: Uuid,
    pub recording_id: i64,
}

impl std::fmt::Display for TimerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}",
            self.addon_id
                .simple(),
            self.recording_id
        )
    }
}

impl std::str::FromStr for TimerId {
    type Err = ();

    fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
        let (addon, recording) = raw
            .split_once(':')
            .ok_or(())?;
        Ok(Self {
            addon_id: Uuid::parse_str(addon).map_err(|_| ())?,
            recording_id: recording
                .parse()
                .map_err(|_| ())?,
        })
    }
}

pub struct DvrService;

impl DvrService {
    /// Timers of the Dispatcharr guide programmes among `rows`.
    pub async fn program_timers(ctx: &AppContext, rows: &[db::Media]) -> ProgramTimers {
        let sources: std::collections::HashSet<&str> = rows
            .iter()
            .filter(|r| {
                r.kind == db::MediaKind::TvProgram
                    && r.external_ids
                        .dispatcharr_program_id
                        .is_some()
            })
            .filter_map(|r| {
                r.external_ids
                    .iptv_source_id
                    .as_deref()
            })
            .collect();
        let mut out = ProgramTimers::default();
        if sources.is_empty() {
            return out;
        }
        for cfg in Self::syncable_configs(ctx).await {
            let source_id = cfg
                .addon_id
                .simple()
                .to_string();
            if !sources.contains(source_id.as_str()) {
                continue;
            }
            let timers = Self::instance_program_timers(&cfg).await;
            out.0
                .extend(
                    timers
                        .iter()
                        .map(|((channel_id, key), ids)| {
                            ((source_id.clone(), *channel_id, key.clone()), ids.clone())
                        }),
                );
        }
        out
    }

    async fn instance_program_timers(cfg: &DvrConfig) -> Arc<ProgramTimerMap> {
        if let Some((at, timers)) = PROGRAM_TIMERS
            .lock()
            .unwrap()
            .get(&cfg.addon_id)
            && at.elapsed() < PROGRAM_TIMERS_TTL
        {
            return timers.clone();
        }
        let client = dispatcharr::CLIENT.clone();
        let timers = match dispatcharr_dvr::list_recordings(
            &client,
            &cfg.base_url,
            &cfg.api_key,
        )
        .await
        {
            Ok(recordings) => {
                let rules = dispatcharr_dvr::list_series_rules(
                    &client,
                    &cfg.base_url,
                    &cfg.api_key,
                )
                .await
                .unwrap_or_default();
                recordings
                    .iter()
                    .filter(|r| {
                        r.status()
                            .is_timer()
                    })
                    .filter_map(|r| {
                        let timer_id = TimerId {
                            addon_id: cfg.addon_id,
                            recording_id: r.id,
                        }
                        .to_string();
                        let series_timer_id = rules
                            .iter()
                            .find(|rule| rule.covers(r))
                            .map(|rule| series_timer_id(cfg.addon_id, rule));
                        Some((
                            (
                                channel_uuid_of(cfg.addon_id, r.channel),
                                r.program_key()?,
                            ),
                            (timer_id, series_timer_id),
                        ))
                    })
                    .collect()
            }
            Err(e) => {
                tracing::warn!(addon = %cfg.addon_id, error = %e, "failed to read Dispatcharr timers for the guide");
                ProgramTimerMap::new()
            }
        };
        let timers = Arc::new(timers);
        PROGRAM_TIMERS
            .lock()
            .unwrap()
            .insert(cfg.addon_id, (std::time::Instant::now(), timers.clone()));
        timers
    }

    /// Makes the next guide read fetch timers afresh, after a DVR change.
    pub fn invalidate_program_timers() {
        PROGRAM_TIMERS
            .lock()
            .unwrap()
            .clear();
    }

    /// Every enabled `dispatcharr`-kind addon instance that is configured
    /// well enough to reach, in addon order.
    pub fn configs(ctx: &AppContext) -> Vec<DvrConfig> {
        ctx.addons
            .list()
            .iter()
            .filter(|r| {
                r.row
                    .enabled
                    && r.row
                        .preset
                        .kind
                        == "dispatcharr"
            })
            .filter_map(|runtime| {
                let config = runtime
                    .row
                    .preset
                    .config
                    .expose();
                Some(DvrConfig {
                    addon_id: runtime
                        .row
                        .id,
                    base_url: config["base_url"]
                        .as_str()
                        .filter(|s| !s.is_empty())?
                        .trim_end_matches('/')
                        .to_string(),
                    api_key: config["api_key"]
                        .as_str()
                        .filter(|s| !s.is_empty())?
                        .to_string(),
                })
            })
            .collect()
    }

    pub fn config_for_addon(ctx: &AppContext, addon_id: Uuid) -> Option<DvrConfig> {
        Self::configs(ctx)
            .into_iter()
            .find(|cfg| cfg.addon_id == addon_id)
    }

    /// The subset of `configs` whose `channels` catalog is enabled.
    async fn syncable_configs(ctx: &AppContext) -> Vec<DvrConfig> {
        let syncable_addons: std::collections::HashSet<Uuid> = ctx
            .addons
            .catalogs_for_kinds(ctx, &[db::MediaKind::TvChannel])
            .await
            .into_iter()
            .filter(|(_, catalogs)| {
                catalogs
                    .iter()
                    .any(|c| c.enabled)
            })
            .map(|(runtime, _)| {
                runtime
                    .row
                    .id
            })
            .collect();
        Self::configs(ctx)
            .into_iter()
            .filter(|cfg| syncable_addons.contains(&cfg.addon_id))
            .collect()
    }

    /// The instance a synced row came from, through the `iptv_source_id` its
    /// sync stamped on it. `None` when that instance is no longer configured.
    pub fn config_for_media(ctx: &AppContext, media: &db::Media) -> Option<DvrConfig> {
        let addon_id = media
            .external_ids
            .iptv_source_id
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok())?;
        Self::config_for_addon(ctx, addon_id)
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

    /// The channel's `(tvg_id, epg_source)`, resolved live through
    /// `epg_data_id -> EPGData`.
    async fn resolve_channel_tvg_id(
        cfg: &DvrConfig,
        channel_id: i64,
    ) -> Result<Option<(String, Option<i64>)>> {
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
            .and_then(|d| {
                d.tvg_id
                    .map(|t| (t, d.epg_source))
            }))
    }

    // -- Timers --------------------------------------------------------

    pub async fn list_timers(
        ctx: &AppContext,
        filter: &TimersFilter,
    ) -> Result<Vec<TimerInfoDto>> {
        let client = dispatcharr::CLIENT.clone();
        let mut timers: Vec<TimerInfoDto> = Vec::new();
        let configs = Self::configs(ctx);
        let mut failed = 0;
        for cfg in &configs {
            // One unreachable instance must not blank the others' timers.
            let recordings = match dispatcharr_dvr::list_recordings(
                &client,
                &cfg.base_url,
                &cfg.api_key,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(addon = %cfg.addon_id, error = %e, "failed to list Dispatcharr timers");
                    failed += 1;
                    continue;
                }
            };
            // Only to say which rule a timer belongs to, so a failure costs
            // the link, not the timers.
            let rules = dispatcharr_dvr::list_series_rules(
                &client,
                &cfg.base_url,
                &cfg.api_key,
            )
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(addon = %cfg.addon_id, error = %e, "failed to list Dispatcharr series rules for timers");
                vec![]
            });
            Self::sync_if_running_unsynced(ctx, cfg, &recordings).await;
            timers.extend(
                recordings
                    .iter()
                    .filter(|r| {
                        r.status()
                            .is_timer()
                    })
                    .map(|r| {
                        let mut timer = timer_from_recording(cfg.addon_id, r);
                        timer.series_timer_id =
                            covering_series_timer(cfg.addon_id, &rules, r);
                        timer
                    })
                    .filter(|t| filter.matches(t)),
            );
        }
        ensure_some_reachable(failed, configs.len())?;
        timers.sort_by_key(|t| t.start_date);
        link_programs(&ctx.db, &mut timers).await?;
        Ok(timers)
    }

    pub async fn get_timer(
        ctx: &AppContext,
        id: TimerId,
    ) -> Result<Option<TimerInfoDto>> {
        let Some(cfg) = Self::config_for_addon(ctx, id.addon_id) else {
            return Ok(None);
        };
        let client = dispatcharr::CLIENT.clone();
        let rec = dispatcharr_dvr::get_recording(
            &client,
            &cfg.base_url,
            &cfg.api_key,
            id.recording_id,
        )
        .await?;
        if let Some(rec) = &rec {
            Self::sync_if_running_unsynced(ctx, &cfg, std::slice::from_ref(rec)).await;
        }
        let Some(rec) = rec.filter(|rec| {
            rec.status()
                .is_timer()
        }) else {
            return Ok(None);
        };
        let mut timer = timer_from_recording(cfg.addon_id, &rec);
        // Only to say which rule the timer belongs to, as in `list_timers`.
        let rules =
            dispatcharr_dvr::list_series_rules(&client, &cfg.base_url, &cfg.api_key)
                .await
                .unwrap_or_default();
        timer.series_timer_id = covering_series_timer(cfg.addon_id, &rules, &rec);
        link_programs(&ctx.db, std::slice::from_mut(&mut timer)).await?;
        Ok(Some(timer))
    }

    pub async fn create_timer(
        ctx: &AppContext,
        req: CreateTimerRequest,
    ) -> Result<TimerInfoDto> {
        let mut program_properties = None;
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
            program_properties = Some(serde_json::json!({
                "id": program
                    .external_ids
                    .dispatcharr_program_id
                    .as_deref()
                    .map(dispatcharr::program_key_value),
                "title": req.name.as_deref().unwrap_or(&program.title),
                "description": program.description,
                "start_time": start.to_rfc3339(),
                "end_time": end.to_rfc3339(),
            }));
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

        // A guide programme goes in as `program`, which Dispatcharr treats as
        // an EPG recording (its own DVR offsets apply, as from its UI). A
        // manual window only carries the client's name, as a plain `title`;
        // Dispatcharr fills the programme from its guide once it starts.
        let mut custom_properties = serde_json::Map::new();
        if let Some(program) = program_properties {
            custom_properties.insert("program".into(), program);
        } else if let Some(name) = &req.name {
            custom_properties.insert("title".into(), serde_json::json!(name));
        }
        custom_properties.insert(
            dispatcharr_dvr::PADDING_KEY.into(),
            dispatcharr_dvr::padding_properties(
                req.pre_padding_seconds,
                req.post_padding_seconds,
            ),
        );

        start -= Duration::seconds(req.pre_padding_seconds as i64);
        end += Duration::seconds(req.post_padding_seconds as i64);

        let channel = db::Media::get_by_id(&ctx.db, &channel_uuid)
            .await?
            .context("channel not found")?;
        let cfg = Self::config_for_media(ctx, &channel)
            .context("channel's Dispatcharr instance not configured")?;

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
            &serde_json::Value::Object(custom_properties),
        )
        .await?;
        let mut timer = timer_from_recording(cfg.addon_id, &rec);
        link_programs(&ctx.db, std::slice::from_mut(&mut timer)).await?;
        Ok(timer)
    }

    /// Re-pads a scheduled timer. Its window is rebuilt from the programme's
    /// own times, so repeated edits replace the padding rather than stack it.
    pub async fn update_timer(
        ctx: &AppContext,
        id: TimerId,
        req: CreateTimerRequest,
    ) -> Result<TimerUpdate> {
        let Some(cfg) = Self::config_for_addon(ctx, id.addon_id) else {
            return Ok(TimerUpdate::NotFound);
        };
        let client = dispatcharr::CLIENT.clone();
        let Some(rec) = dispatcharr_dvr::get_recording(
            &client,
            &cfg.base_url,
            &cfg.api_key,
            id.recording_id,
        )
        .await?
        else {
            return Ok(TimerUpdate::NotFound);
        };
        match rec.status() {
            dispatcharr_dvr::DispatcharrRecordingStatus::Scheduled => {}
            dispatcharr_dvr::DispatcharrRecordingStatus::Recording => {
                return Ok(TimerUpdate::AlreadyStarted);
            }
            _ => return Ok(TimerUpdate::NotFound),
        }
        let (start, end) = programme_window(&rec, req.start_date, req.end_date);
        dispatcharr_dvr::update_recording(
            &client,
            &cfg.base_url,
            &cfg.api_key,
            rec.id,
            start - Duration::seconds(req.pre_padding_seconds as i64),
            end + Duration::seconds(req.post_padding_seconds as i64),
            &dispatcharr_dvr::with_padding(
                &rec,
                req.pre_padding_seconds,
                req.post_padding_seconds,
            ),
        )
        .await?;
        Ok(TimerUpdate::Updated)
    }

    /// The instance and recording a timer id as a client sends it refers to:
    /// our own `{addon}:{recording}` form, or the UUID of the in-progress
    /// `Recording` item a timer points at through `ProgramInfo` (some clients
    /// cancel by `ProgramInfo.Id`).
    pub async fn resolve_timer_id(ctx: &AppContext, raw: &str) -> Option<TimerId> {
        if let Ok(id) = raw.parse::<TimerId>() {
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
            if media.kind != db::MediaKind::Recording {
                return None;
            }
            let cfg = Self::config_for_media(ctx, &media)?;
            return media
                .external_ids
                .dispatcharr_recording_id
                .map(|recording_id| TimerId {
                    addon_id: cfg.addon_id,
                    recording_id,
                });
        }
        // Not synced yet (a client can cancel from the schedule before it ever
        // lists recordings): the id is derived from Dispatcharr's own, so match
        // it against each instance's live list.
        for cfg in Self::configs(ctx) {
            let Ok(recordings) = dispatcharr_dvr::list_recordings(
                &dispatcharr::CLIENT,
                &cfg.base_url,
                &cfg.api_key,
            )
            .await
            else {
                continue;
            };
            if let Some(recording_id) =
                recording_id_for_uuid(&recordings, cfg.addon_id, uuid)
            {
                return Some(TimerId {
                    addon_id: cfg.addon_id,
                    recording_id,
                });
            }
        }
        None
    }

    /// Cancels the timer: stops it if it's currently recording (keeping the
    /// partial file), deletes it if it hasn't started yet. A finished entry
    /// is a recording, not a timer, so it is left alone.
    pub async fn delete_timer(ctx: &AppContext, id: TimerId) -> Result<bool> {
        let Some(cfg) = Self::config_for_addon(ctx, id.addon_id) else {
            return Ok(false);
        };
        let client = dispatcharr::CLIENT.clone();
        let rec_id = id.recording_id;
        let Some(rec) = dispatcharr_dvr::get_recording(
            &client,
            &cfg.base_url,
            &cfg.api_key,
            rec_id,
        )
        .await?
        else {
            return Ok(false);
        };
        match rec.status() {
            dispatcharr_dvr::DispatcharrRecordingStatus::Recording => {
                dispatcharr_dvr::stop_recording(
                    &client,
                    &cfg.base_url,
                    &cfg.api_key,
                    rec_id,
                )
                .await?;
            }
            dispatcharr_dvr::DispatcharrRecordingStatus::Scheduled => {
                dispatcharr_dvr::delete_recording(
                    &client,
                    &cfg.base_url,
                    &cfg.api_key,
                    rec_id,
                )
                .await?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Syncs `cfg`'s recordings when one of `recordings` is running without a
    /// row yet, so the item a running timer's `ProgramInfo` names exists.
    async fn sync_if_running_unsynced(
        ctx: &AppContext,
        cfg: &DvrConfig,
        recordings: &[dispatcharr_dvr::DispatcharrRecording],
    ) {
        let mut unsynced = false;
        for rec in recordings
            .iter()
            .filter(|r| {
                r.status() == dispatcharr_dvr::DispatcharrRecordingStatus::Recording
            })
        {
            let id = dispatcharr::recording_media_id(cfg.addon_id, rec.id);
            if matches!(db::Media::get_by_id(&ctx.db, &id).await, Ok(None)) {
                unsynced = true;
                break;
            }
        }
        if !unsynced
            || !Self::syncable_configs(ctx)
                .await
                .iter()
                .any(|c| c.addon_id == cfg.addon_id)
        {
            return;
        }
        if let Err(e) = Self::sync_recordings(ctx, cfg).await {
            tracing::warn!(addon = %cfg.addon_id, error = %e, "failed to sync a running recording");
        }
    }

    // -- SeriesTimers ----------------------------------------------------

    pub async fn list_series_timers(
        ctx: &AppContext,
    ) -> Result<Vec<SeriesTimerInfoDto>> {
        let client = dispatcharr::CLIENT.clone();
        let mut timers = Vec::new();
        let configs = Self::configs(ctx);
        let mut failed = 0;
        for cfg in &configs {
            // One unreachable instance must not blank the others' rules.
            match dispatcharr_dvr::list_series_rules(
                &client,
                &cfg.base_url,
                &cfg.api_key,
            )
            .await
            {
                Ok(rules) => timers.extend(
                    rules
                        .iter()
                        .map(|r| series_timer_from_rule(cfg.addon_id, r)),
                ),
                Err(e) => {
                    tracing::warn!(addon = %cfg.addon_id, error = %e, "failed to list Dispatcharr series rules");
                    failed += 1;
                }
            }
        }
        ensure_some_reachable(failed, configs.len())?;
        Ok(timers)
    }

    /// Matches `id` against the listed series timers whether or not it is
    /// still percent-encoded.
    pub async fn get_series_timer(
        ctx: &AppContext,
        id: &str,
    ) -> Result<Option<SeriesTimerInfoDto>> {
        let Some(target) = parse_series_timer_id(id) else {
            return Ok(None);
        };
        let items = Self::list_series_timers(ctx).await?;
        Ok(items
            .into_iter()
            .find(|t| parse_series_timer_id(&t.id) == Some(target.clone())))
    }

    pub async fn create_series_timer(
        ctx: &AppContext,
        req: CreateSeriesTimerRequest,
    ) -> Result<SeriesTimerInfoDto> {
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

        let channel = db::Media::get_by_id(&ctx.db, &channel_uuid)
            .await?
            .context("channel not found")?;
        let cfg = Self::config_for_media(ctx, &channel)
            .context("channel's Dispatcharr instance not configured")?;

        let channel_id = Self::resolve_channel_int(&cfg, channel_uuid)
            .await?
            .context("channel not found on Dispatcharr")?;
        let (tvg_id, epg_source_id) = Self::resolve_channel_tvg_id(&cfg, channel_id)
            .await?
            .context("channel has no EPG mapping on Dispatcharr")?;

        // Pinned to the channel's own guide source: without one Dispatcharr
        // reads the rule as matching every mapped guide sharing the tvg_id.
        let request = dispatcharr_dvr::SeriesRuleRequest {
            tvg_id: Some(tvg_id.clone()),
            mode: series_mode(req.record_new_only).to_string(),
            title: Some(name),
            epg_source_id,
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
        let timer = series_timer_from_rule(cfg.addon_id, created);
        Self::evaluate_series_rules(&cfg, &tvg_id).await;
        Ok(timer)
    }

    /// Applies an edit to a series timer: its name, channel and new-only
    /// setting. Anything else a Dispatcharr rule cannot hold is refused when
    /// it differs from what the timer reports.
    /// The instance and Dispatcharr rule a series timer id names, if it still
    /// exists.
    async fn find_series_rule(
        ctx: &AppContext,
        id: &str,
    ) -> Result<Option<(DvrConfig, dispatcharr_dvr::DispatcharrSeriesRule)>> {
        let Some(wanted) = parse_series_timer_id(id) else {
            return Ok(None);
        };
        let Some(cfg) = Self::config_for_addon(ctx, wanted.0) else {
            return Ok(None);
        };
        let rules = dispatcharr_dvr::list_series_rules(
            &dispatcharr::CLIENT,
            &cfg.base_url,
            &cfg.api_key,
        )
        .await?;
        let rule = rules
            .into_iter()
            .find(|r| {
                parse_series_timer_id(&series_timer_id(cfg.addon_id, r))
                    == Some(wanted.clone())
            });
        Ok(rule.map(|r| (cfg, r)))
    }

    /// Applies `RecordNewOnly`, the one series timer setting a Dispatcharr
    /// rule can change in place. Returns `false` when the rule is gone.
    pub async fn update_series_timer(
        ctx: &AppContext,
        id: &str,
        req: UpdateSeriesTimerRequest,
    ) -> Result<bool> {
        let Some((cfg, rule)) = Self::find_series_rule(ctx, id).await? else {
            return Ok(false);
        };
        let Some(mode) = req
            .record_new_only
            .map(series_mode)
            .filter(|m| *m != rule.mode)
        else {
            return Ok(true);
        };
        dispatcharr_dvr::create_series_rule(
            &dispatcharr::CLIENT,
            &cfg.base_url,
            &cfg.api_key,
            &rule.with_mode(mode),
        )
        .await?;
        if let Some(tvg_id) = &rule.tvg_id {
            Self::evaluate_series_rules(&cfg, tvg_id).await;
        }
        Ok(true)
    }

    /// Best effort: the rule is saved either way, and Dispatcharr evaluates
    /// every rule again after its next guide refresh.
    async fn evaluate_series_rules(cfg: &DvrConfig, tvg_id: &str) {
        if let Err(e) = dispatcharr_dvr::evaluate_series_rules(
            &dispatcharr::CLIENT,
            &cfg.base_url,
            &cfg.api_key,
            tvg_id,
        )
        .await
        {
            tracing::warn!(addon = %cfg.addon_id, tvg_id, error = %e, "failed to evaluate Dispatcharr series rules");
        }
    }

    pub async fn delete_series_timer(ctx: &AppContext, id: &str) -> Result<bool> {
        let Some((cfg, rule)) = Self::find_series_rule(ctx, id).await? else {
            return Ok(false);
        };
        dispatcharr_dvr::delete_series_rule(
            &dispatcharr::CLIENT,
            &cfg.base_url,
            &cfg.api_key,
            rule.tvg_id
                .as_deref(),
            rule.title
                .as_deref(),
            rule.epg_source_id,
        )
        .await?;
        Ok(true)
    }

    // -- Recordings ------------------------------------------------------

    /// Makes sure every channel row the given recordings hang off exists,
    /// importing the ones that are missing. Returns the channel ids that are
    /// actually present afterwards — a recording on a channel Dispatcharr no
    /// longer lists has no row to hang off and cannot be synced.
    async fn ensure_channel_parents(
        ctx: &AppContext,
        cfg: &DvrConfig,
        recordings: &[dispatcharr_dvr::DispatcharrRecording],
    ) -> Result<std::collections::HashSet<Uuid>> {
        let wanted: std::collections::HashSet<Uuid> = recordings
            .iter()
            .map(|r| channel_uuid_of(cfg.addon_id, r.channel))
            .collect();
        if wanted.is_empty() {
            return Ok(wanted);
        }

        let wanted_ids: Vec<Uuid> = wanted
            .iter()
            .copied()
            .collect();
        let mut present: std::collections::HashSet<Uuid> =
            std::collections::HashSet::new();
        for chunk in wanted_ids.chunks(500) {
            let mut qb = sqlx::QueryBuilder::new("SELECT id FROM media WHERE id IN (");
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            qb.push(")");
            let found: Vec<Uuid> = qb
                .build_query_scalar()
                .fetch_all(&ctx.db)
                .await?;
            present.extend(found);
        }
        if present.len() == wanted.len() {
            return Ok(present);
        }

        let source_id = cfg
            .addon_id
            .simple()
            .to_string();
        let mut channels = dispatcharr::fetch_channels(
            &dispatcharr::CLIENT,
            &cfg.base_url,
            &cfg.api_key,
        )
        .await?;
        channels.truncate(
            dispatcharr::channel_limit(ctx, cfg.addon_id)
                .await
                .unwrap_or(0),
        );
        let missing: Vec<db::Media> = channels
            .iter()
            .map(|ch| dispatcharr::channel_to_media(ch, cfg.addon_id, &source_id))
            .filter(|m| wanted.contains(&m.id) && !present.contains(&m.id))
            .collect();
        present.extend(
            missing
                .iter()
                .map(|m| m.id),
        );
        db::Media::upsert(&ctx.db, &missing).await?;
        Ok(present)
    }

    /// Upserts one instance's started recordings as `Recording` rows with
    /// their `Stream` children, and prunes the rest. Returns how many synced.
    pub async fn sync_recordings(ctx: &AppContext, cfg: &DvrConfig) -> Result<usize> {
        Ok(Self::sync_recording_statuses(ctx, cfg)
            .await?
            .len())
    }

    /// `sync_recordings`, returning each synced row's upstream status.
    async fn sync_recording_statuses(
        ctx: &AppContext,
        cfg: &DvrConfig,
    ) -> Result<HashMap<Uuid, RecordingStatus>> {
        let _guard = SYNC_RECORDINGS_LOCKS
            .lock(cfg.addon_id)
            .await;
        let client = dispatcharr::CLIENT.clone();
        let source_id = cfg
            .addon_id
            .simple()
            .to_string();

        let mut recordings =
            dispatcharr_dvr::list_recordings(&client, &cfg.base_url, &cfg.api_key)
                .await?;
        // Not-yet-started recordings have no file yet; they are Timers only.
        recordings.retain(|r| {
            !matches!(
                r.status(),
                dispatcharr_dvr::DispatcharrRecordingStatus::Scheduled
            )
        });

        // Each row needs its channel row as FK parent; recordings on a
        // channel Dispatcharr no longer lists are skipped.
        let parents = Self::ensure_channel_parents(ctx, cfg, &recordings).await?;
        let before = recordings.len();
        recordings
            .retain(|r| parents.contains(&channel_uuid_of(cfg.addon_id, r.channel)));
        if recordings.len() < before {
            tracing::warn!(
                addon = %cfg.addon_id,
                dropped = before - recordings.len(),
                "skipping Dispatcharr recordings whose channel no longer exists"
            );
        }

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
        Self::delete_replaced_recording_streams(ctx, &stream_rows).await?;

        let keep_ids: std::collections::HashSet<Uuid> = recording_rows
            .iter()
            .map(|m| m.id)
            .collect();
        let synced: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM media WHERE kind = 'recording' AND json_extract(external_ids, '$.iptv_source_id') = ?",
        )
        .bind(&source_id)
        .fetch_all(&ctx.db)
        .await?;
        let gone: Vec<Uuid> = synced
            .into_iter()
            .filter(|id| !keep_ids.contains(id))
            .collect();
        for chunk in gone.chunks(SQL_BIND_CHUNK) {
            let mut qb = sqlx::QueryBuilder::new("DELETE FROM media WHERE id IN (");
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            qb.push(")");
            qb.build()
                .execute(&ctx.db)
                .await?;
        }

        Ok(recording_rows
            .iter()
            .zip(recordings.iter())
            .map(|(media, rec)| {
                (
                    media.id,
                    rec.status()
                        .into(),
                )
            })
            .collect())
    }

    /// Deletes each synced recording's other `Stream` child, left from before
    /// it finished, along with the probe cached for it.
    async fn delete_replaced_recording_streams(
        ctx: &AppContext,
        stream_rows: &[db::Media],
    ) -> Result<()> {
        // Two binds per row.
        for chunk in stream_rows.chunks(SQL_BIND_CHUNK / 2) {
            let mut qb = sqlx::QueryBuilder::new(
                "DELETE FROM media WHERE kind = 'stream' AND parent_id IN (",
            );
            let mut sep = qb.separated(", ");
            for row in chunk {
                sep.push_bind(row.parent_id);
            }
            qb.push(") AND id NOT IN (");
            let mut sep = qb.separated(", ");
            for row in chunk {
                sep.push_bind(row.id);
            }
            qb.push(")");
            qb.build()
                .execute(&ctx.db)
                .await?;
        }
        Ok(())
    }

    pub async fn list_recordings(
        ctx: &AppContext,
        filter: &RecordingsFilter,
    ) -> Result<Vec<api::BaseItemDto>> {
        let mut statuses = HashMap::new();
        for cfg in Self::syncable_configs(ctx).await {
            match Self::sync_recording_statuses(ctx, &cfg).await {
                Ok(s) => statuses.extend(s),
                Err(e) => {
                    tracing::warn!(addon = %cfg.addon_id, error = %e, "failed to refresh recordings from Dispatcharr, serving last-synced state");
                }
            }
        }
        let now = Utc::now().naive_utc();
        let mut result = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Recording]),
                ..Default::default()
            },
        )
        .await?;
        result
            .records
            .retain(|m| {
                let status = statuses
                    .get(&m.id)
                    .copied()
                    .unwrap_or_else(|| status_from_schedule(m, now));
                filter.matches(m, status)
            });
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
            // Not synced yet (e.g. it just finished): sync each instance
            // once and look again.
            for cfg in Self::syncable_configs(ctx).await {
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

    /// Deletes the recording on its instance and the synced row. With that
    /// instance no longer configured, only the row goes.
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
            Self::config_for_media(ctx, &media),
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
    /// Dispatcharr config to reach it with, plus its current
    /// `DispatcharrRecording`, fetched live for an up-to-date `file_url` and
    /// status.
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
        let Some(cfg) = Self::config_for_media(ctx, &media) else {
            return Ok(None);
        };
        let Some(rec) = dispatcharr_dvr::get_recording(
            &dispatcharr::CLIENT,
            &cfg.base_url,
            &cfg.api_key,
            dispatcharr_id,
        )
        .await?
        else {
            return Ok(None);
        };
        Ok(Some((cfg, rec)))
    }
}

/// Which timers a `/livetv/timers` query asks for.
#[derive(Debug, Default, Clone)]
pub struct TimersFilter {
    pub channel_id: Option<Uuid>,
    pub series_timer_id: Option<String>,
    /// In progress.
    pub is_active: Option<bool>,
    /// Not started yet.
    pub is_scheduled: Option<bool>,
}

impl TimersFilter {
    fn matches(&self, timer: &TimerInfoDto) -> bool {
        self.channel_id
            .is_none_or(|c| timer.channel_id == Some(c))
            && self
                .series_timer_id
                .as_deref()
                .is_none_or(|wanted| {
                    timer
                        .series_timer_id
                        .as_deref()
                        .is_some_and(|id| {
                            parse_series_timer_id(id) == parse_series_timer_id(wanted)
                        })
                })
            && self
                .is_active
                .is_none_or(|a| a == (timer.status == RecordingStatus::InProgress))
            && self
                .is_scheduled
                .is_none_or(|s| s == (timer.status == RecordingStatus::New))
    }
}

/// Outcome of `DvrService::update_timer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerUpdate {
    Updated,
    NotFound,
    /// Recording has begun, so its start can no longer move.
    AlreadyStarted,
}

/// Which synced recordings a `/livetv/recordings` query asks for.
#[derive(Debug, Default, Clone)]
pub struct RecordingsFilter {
    pub channel_id: Option<Uuid>,
    pub status: Option<RecordingStatus>,
    pub is_in_progress: Option<bool>,
}

impl RecordingsFilter {
    fn matches(&self, media: &db::Media, status: RecordingStatus) -> bool {
        self.channel_id
            .is_none_or(|c| media.parent_id == Some(c))
            && self
                .status
                .is_none_or(|s| s == status)
            && self
                .is_in_progress
                .is_none_or(|p| p == (status == RecordingStatus::InProgress))
    }
}

/// Best guess at a recording's status when its instance couldn't be asked:
/// in progress while inside its scheduled window. A recording stopped early
/// keeps its scheduled `end_time` upstream, so this can overstate it until the
/// next sync records the real end.
fn status_from_schedule(
    media: &db::Media,
    now: chrono::NaiveDateTime,
) -> RecordingStatus {
    let started = media
        .live_start
        .is_some_and(|s| s <= now);
    let ended = media
        .live_end
        .is_none_or(|e| e <= now);
    if started && !ended {
        RecordingStatus::InProgress
    } else {
        RecordingStatus::Completed
    }
}

/// Newest recording first, the order clients expect for "recent recordings".
/// Items without a parseable start go last.
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

/// A timer's programme times: those Dispatcharr stored for the programme,
/// then the given ones, then the recording's window with remux's padding
/// taken back off.
fn programme_window(
    rec: &dispatcharr_dvr::DispatcharrRecording,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    if let Some(window) = rec.program_window() {
        return window;
    }
    let (pre, post) = rec.padding();
    (
        start.unwrap_or(rec.start_time + Duration::seconds(pre as i64)),
        end.unwrap_or(rec.end_time - Duration::seconds(post as i64)),
    )
}

fn series_mode(record_new_only: bool) -> &'static str {
    if record_new_only { "new" } else { "all" }
}

/// Points each timer's `program_id` at its programme in the synced guide: the
/// row on the timer's channel carrying the recording's Dispatcharr programme
/// id, else the row keyed by the programme's start and title. Cleared when the
/// guide has neither.
async fn link_programs(
    db: &sqlx::SqlitePool,
    timers: &mut [TimerInfoDto],
) -> Result<()> {
    let keys: Vec<&str> = timers
        .iter()
        .filter_map(|t| {
            t.dispatcharr_program_id
                .as_deref()
        })
        .collect();
    let mut by_key: HashMap<(String, Uuid), Uuid> = HashMap::new();
    for chunk in keys.chunks(500) {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT id, parent_id, json_extract(external_ids, '$.dispatcharr_program_id') \
             FROM media WHERE kind = 'tv_program' \
             AND json_extract(external_ids, '$.dispatcharr_program_id') IN (",
        );
        let mut sep = qb.separated(", ");
        for key in chunk {
            sep.push_bind(*key);
        }
        qb.push(")");
        let rows: Vec<(Uuid, Option<Uuid>, String)> = qb
            .build_query_as()
            .fetch_all(db)
            .await?;
        for (id, parent_id, key) in rows {
            if let Some(channel) = parent_id {
                by_key.insert((key, channel), id);
            }
        }
    }

    let derived: Vec<Uuid> = timers
        .iter()
        .filter_map(|t| t.program_id)
        .collect();
    let mut known = std::collections::HashSet::new();
    for chunk in derived.chunks(500) {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT id FROM media WHERE kind = 'tv_program' AND id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(id);
        }
        qb.push(")");
        let found: Vec<Uuid> = qb
            .build_query_scalar()
            .fetch_all(db)
            .await?;
        known.extend(found);
    }

    for timer in timers {
        let by_program_key = timer
            .dispatcharr_program_id
            .clone()
            .zip(timer.channel_id)
            .and_then(|k| by_key.get(&k))
            .copied();
        timer.program_id = by_program_key.or(timer
            .program_id
            .filter(|id| known.contains(id)));
    }
    Ok(())
}

/// How long a guide read reuses an instance's pending and active recordings.
const PROGRAM_TIMERS_TTL: std::time::Duration = std::time::Duration::from_secs(30);

type ProgramTimerMap = HashMap<(Uuid, String), (String, Option<String>)>;

/// Per instance: when it was read, and `(channel, programme key) -> (timer
/// id, series timer id)` for the recordings it has pending or running.
static PROGRAM_TIMERS: std::sync::LazyLock<
    std::sync::Mutex<HashMap<Uuid, (std::time::Instant, Arc<ProgramTimerMap>)>>,
> = std::sync::LazyLock::new(Default::default);

/// The timer and series timer of each guide programme due to be recorded or
/// being recorded, for setting `TimerId`/`SeriesTimerId` on guide items.
#[derive(Default)]
pub struct ProgramTimers(HashMap<(String, Uuid, String), (String, Option<String>)>);

impl ProgramTimers {
    /// `row` as an item, carrying its timer ids when it is being recorded.
    pub fn to_item(&self, row: db::Media) -> api::BaseItemDto {
        let ids = match (
            row.external_ids
                .iptv_source_id
                .clone(),
            row.parent_id,
            row.external_ids
                .dispatcharr_program_id
                .clone(),
        ) {
            (Some(source_id), Some(channel_id), Some(program_id)) => self
                .0
                .get(&(source_id, channel_id, program_id))
                .cloned(),
            _ => None,
        };
        let mut item = api::db_media_to_item(row, false);
        if let Some((timer_id, series_timer_id)) = ids {
            item.timer_id = Some(timer_id);
            item.series_timer_id = series_timer_id;
        }
        item
    }
}

/// The id of the first of `rules` that schedules `rec`.
fn covering_series_timer(
    addon_id: Uuid,
    rules: &[dispatcharr_dvr::DispatcharrSeriesRule],
    rec: &dispatcharr_dvr::DispatcharrRecording,
) -> Option<String> {
    rules
        .iter()
        .find(|rule| rule.covers(rec))
        .map(|rule| series_timer_id(addon_id, rule))
}

fn timer_from_recording(
    addon_id: Uuid,
    rec: &dispatcharr_dvr::DispatcharrRecording,
) -> TimerInfoDto {
    let (start_date, end_date) = programme_window(rec, None, None);
    let (pre_padding_seconds, post_padding_seconds) = rec.padding();
    TimerInfoDto {
        id: TimerId {
            addon_id,
            recording_id: rec.id,
        }
        .to_string(),
        type_: "Timer".to_string(),
        server_id: crate::common::server_id(),
        channel_id: Some(channel_uuid_of(addon_id, rec.channel)),
        program_id: rec
            .program_start_and_title()
            .map(|(start, title)| {
                dispatcharr::program_media_id(
                    channel_uuid_of(addon_id, rec.channel),
                    start,
                    title,
                )
            }),
        name: rec
            .program_title()
            .unwrap_or("Recording")
            .to_string(),
        overview: rec
            .program_description()
            .map(str::to_owned),
        start_date,
        end_date,
        service_name: "dispatcharr".to_string(),
        priority: 0,
        pre_padding_seconds,
        post_padding_seconds,
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
        dispatcharr_program_id: rec.program_key(),
    }
}

fn series_timer_id(
    addon_id: Uuid,
    rule: &dispatcharr_dvr::DispatcharrSeriesRule,
) -> String {
    let raw = format!(
        "{}|{}|{}|{}",
        addon_id.simple(),
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

/// `(addon, tvg_id, title, epg_source_id)`.
fn parse_series_timer_id(
    id: &str,
) -> Option<(Uuid, Option<String>, Option<String>, Option<i64>)> {
    let decoded = urlencoding::decode(id).ok()?;
    let (addon_id, decoded) = decoded.split_once('|')?;
    let addon_id = Uuid::parse_str(addon_id).ok()?;
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
    Some((addon_id, tvg_id, title, epg_source_id))
}

/// An error if every one of `total` configured instances failed, so an
/// unreachable Dispatcharr reads as an outage rather than an empty schedule.
/// Some failing is tolerated: the others' entries are still worth showing.
fn ensure_some_reachable(failed: usize, total: usize) -> Result<()> {
    if total > 0 && failed == total {
        anyhow::bail!("no Dispatcharr instance could be reached");
    }
    Ok(())
}

fn series_timer_from_rule(
    addon_id: Uuid,
    rule: &dispatcharr_dvr::DispatcharrSeriesRule,
) -> SeriesTimerInfoDto {
    SeriesTimerInfoDto {
        id: series_timer_id(addon_id, rule),
        type_: "SeriesTimer".to_string(),
        server_id: crate::common::server_id(),
        channel_id: Uuid::nil(),
        name: rule
            .title
            .clone()
            .unwrap_or_else(|| "All programs".to_string()),
        overview: rule
            .description
            .clone(),
        start_date: dotnet_min_value(),
        end_date: dotnet_min_value(),
        service_name: "dispatcharr".to_string(),
        priority: 0,
        is_pre_padding_required: false,
        is_post_padding_required: false,
        keep_until: "UntilDeleted",
        record_any_time: true,
        record_any_channel: rule
            .tvg_id
            .is_none(),
        record_new_only: rule.mode == "new",
        skip_episodes_in_library: false,
        pre_padding_seconds: 0,
        post_padding_seconds: 0,
        keep_up_to: 0,
        days: ALL_DAYS
            .iter()
            .map(|d| d.to_string())
            .collect(),
    }
}

/// `0001-01-01T00:00:00Z`, .NET's `DateTime.MinValue`. chrono's own minimum
/// serialises to a year 0 .NET clients can't parse.
fn dotnet_min_value() -> DateTime<Utc> {
    chrono::NaiveDate::from_ymd_opt(1, 1, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc())
        .unwrap_or_default()
}

const ALL_DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

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
    ConflictedOk,
    ConflictedNotOk,
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
    #[serde(with = "remux_utils::uuid_serde::opt")]
    pub channel_id: Option<Uuid>,
    #[serde(with = "remux_utils::uuid_serde::opt")]
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
    /// Only while the recording is in progress: the playable `Recording` item,
    /// so a client can open a running timer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program_info: Option<api::BaseItemDto>,
    /// Dispatcharr's id of the programme being recorded, for `link_programs`.
    #[serde(skip)]
    pub dispatcharr_program_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SeriesTimerInfoDto {
    pub id: String,
    #[serde(rename = "Type")]
    pub type_: String,
    pub server_id: String,
    /// Jellyfin's empty Guid: a rule is pinned to a guide id, not a channel.
    pub channel_id: Uuid,
    pub name: String,
    pub overview: Option<String>,
    /// A rule has no airing of its own, so both dates are .NET's `MinValue`.
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub service_name: String,
    pub priority: i32,
    pub is_pre_padding_required: bool,
    pub is_post_padding_required: bool,
    pub keep_until: &'static str,
    pub record_any_time: bool,
    pub record_any_channel: bool,
    pub record_new_only: bool,
    pub skip_episodes_in_library: bool,
    pub pre_padding_seconds: i32,
    pub post_padding_seconds: i32,
    pub keep_up_to: i32,
    pub days: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateTimerRequest {
    pub program_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    #[serde(default)]
    pub name: Option<String>,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub pre_padding_seconds: i32,
    #[serde(default)]
    pub post_padding_seconds: i32,
}

/// An edit to a series timer, in `SeriesTimerInfoDto`'s shape. Only
/// `RecordNewOnly` is applied; a Dispatcharr rule holds nothing else a client
/// can edit.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateSeriesTimerRequest {
    pub record_new_only: Option<bool>,
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
    use crate::{
        addons::dispatcharr_dvr::{
            DispatcharrRecording, DispatcharrRecordingStatus as Status,
            DispatcharrSeriesRule,
        },
        integration_test::{TestGuard, new_test_server},
    };
    use httpmock::{
        Method::{DELETE, GET, PATCH, POST},
        Mock, MockServer,
    };
    use serde_json::{Value, json};

    const ADDON: Uuid = Uuid::from_u128(0xd15b);

    fn recording(id: i64, custom_properties: Value) -> DispatcharrRecording {
        serde_json::from_value(rec_json_with(id, 42, custom_properties)).unwrap()
    }

    fn rec_json_with(id: i64, channel: i64, custom_properties: Value) -> Value {
        json!({
            "id": id, "channel": channel,
            "start_time": "2026-09-15T20:00:00Z",
            "end_time": "2026-09-15T21:00:00Z",
            "custom_properties": custom_properties,
        })
    }

    fn rec_json(id: i64, status: Option<&str>) -> Value {
        let props = match status {
            Some(s) => {
                json!({ "status": s, "program": { "title": format!("Show {id}") } })
            }
            None => json!({}),
        };
        rec_json_with(id, 42, props)
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
            ..Default::default()
        }
    }

    fn news_rules() -> Value {
        json!({ "rules": [
            { "tvg_id": "BBC1.uk", "title": "News", "mode": "all", "epg_source_id": 3 },
        ]})
    }

    fn timer(addon_id: Uuid, recording_id: i64) -> TimerId {
        TimerId {
            addon_id,
            recording_id,
        }
    }

    fn rid(n: i64) -> Uuid {
        Uuid::new_v5(&ADDON, format!("recording:{n}").as_bytes())
    }

    fn source(addon: Uuid) -> String {
        addon
            .simple()
            .to_string()
    }

    /// A test server with Dispatcharr instance `ADDON` served by `mock`.
    struct Env {
        _server: axum_test::TestServer,
        guard: TestGuard,
        mock: MockServer,
    }

    impl Env {
        /// `ADDON` registered, with its channel 42 already synced.
        async fn new() -> Self {
            let env = Self::without_channels().await;
            seed_channel(env.ctx(), ADDON).await;
            env
        }

        async fn without_channels() -> Self {
            let (server, guard) = new_test_server()
                .await
                .unwrap();
            let mock = MockServer::start();
            register_dispatcharr(&guard.0, ADDON, &mock.base_url()).await;
            Self {
                _server: server,
                guard,
                mock,
            }
        }

        fn ctx(&self) -> &AppContext {
            &self
                .guard
                .0
        }

        fn cfg(&self) -> DvrConfig {
            DvrConfig {
                addon_id: ADDON,
                base_url: self
                    .mock
                    .base_url(),
                api_key: "k".into(),
            }
        }

        fn json(&self, method: httpmock::Method, path: &str, body: Value) -> Mock<'_> {
            self.mock
                .mock(|when, then| {
                    when.method(method)
                        .path(path);
                    then.status(200)
                        .json_body(body);
                })
        }

        fn status(
            &self,
            method: httpmock::Method,
            path: &str,
            status: u16,
        ) -> Mock<'_> {
            self.mock
                .mock(|when, then| {
                    when.method(method)
                        .path(path);
                    then.status(status);
                })
        }

        fn channels(&self) -> Mock<'_> {
            self.json(
                GET,
                "/api/channels/channels/",
                json!([{ "id": 42, "uuid": "u", "name": "BBC One", "epg_data_id": 5 }]),
            )
        }
    }

    async fn seed_channel(ctx: &AppContext, addon: Uuid) {
        let ch: dispatcharr::DispatcharrChannel =
            serde_json::from_value(json!({ "id": 42, "uuid": "u", "name": "BBC One" }))
                .unwrap();
        db::Media::upsert(
            &ctx.db,
            &vec![dispatcharr::channel_to_media(&ch, addon, &source(addon))],
        )
        .await
        .unwrap();
    }

    /// Registers `addon` as an enabled Dispatcharr instance pointed at
    /// `base_url`, so `configs()` can resolve it.
    async fn register_dispatcharr(ctx: &AppContext, addon: Uuid, base_url: &str) {
        let now = chrono::Utc::now().naive_utc();
        let cfg = json!({ "base_url": base_url, "api_key": "k" });
        let row = crate::addons::Addon {
            id: addon,
            name: format!("dispatcharr-{addon}"),
            preset: crate::addons::AddonPresetRef {
                kind: "dispatcharr".into(),
                config: cfg
                    .clone()
                    .into(),
            },
            resources: vec![],
            types: vec![],
            enabled: true,
            priority: 0,
            created_at: now,
            updated_at: now,
            system: false,
            is_default: true,
            http_redirect_stream: false,
            service_filter: vec![],
        };
        row.insert(&ctx.db)
            .await
            .unwrap();
        // Real caps, not `Default::default()`: `syncable_configs` walks the
        // `channels` catalog through `AddonRuntime.caps.catalog`, which a
        // default-constructed runtime has none of.
        use crate::addons::AddonPreset as _;
        let caps = crate::addons::dispatcharr::DispatcharrPreset
            .from_cfg(addon, &cfg, &ctx.config)
            .unwrap();
        let mut runtimes: Vec<crate::addons::AddonRuntime> = ctx
            .addons
            .list()
            .iter()
            .cloned()
            .collect();
        runtimes.push(crate::addons::AddonRuntime { row, caps });
        ctx.addons
            .replace_runtimes_for_test(runtimes);
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

    // -- ids ----------------------------------------------------------

    #[test]
    fn series_timer_ids_round_trip() {
        let id = series_timer_id(
            ADDON,
            &rule(Some("BBC1.uk"), Some("Match of the Day"), Some(3)),
        );
        assert_eq!(
            id,
            format!("{}%7CBBC1.uk%7CMatch%20of%20the%20Day%7C3", ADDON.simple())
        );
        for (tvg, title, epg) in [
            (Some("BBC1.uk"), Some("Match of the Day"), Some(3)),
            (Some("BBC1.uk"), Some("Match of the Day"), None),
            (None, Some("Any channel"), None),
            (Some("BBC1.uk"), None, None),
            (None, None, None),
            (Some("a b"), Some("Q&A: 50% off"), Some(12)),
            (Some("BBC1.uk"), Some("Cats | Dogs"), Some(3)),
        ] {
            let id = series_timer_id(ADDON, &rule(tvg, title, epg));
            let want = (ADDON, tvg.map(str::to_owned), title.map(str::to_owned), epg);
            assert_eq!(parse_series_timer_id(&id), Some(want), "{id}");
        }
        // axum percent-decodes path params before the handler sees them.
        assert_eq!(
            parse_series_timer_id(&format!("{}|BBC1.uk|MOTD|3", ADDON.simple())),
            Some((ADDON, Some("BBC1.uk".into()), Some("MOTD".into()), Some(3)))
        );
        assert_eq!(
            parse_series_timer_id(&format!("{}|BBC1.uk|MOTD|abc", ADDON.simple())),
            Some((ADDON, Some("BBC1.uk".into()), Some("MOTD".into()), None))
        );
        assert_eq!(parse_series_timer_id("BBC1.uk|MOTD|3"), None, "no instance");
        assert_eq!(parse_series_timer_id("%FF%FE"), None);
    }

    #[test]
    fn a_timer_id_names_the_instance_it_lives_on() {
        let id = timer(ADDON, 7);
        assert_eq!(id.to_string(), format!("{}:7", ADDON.simple()));
        assert_eq!(
            id.to_string()
                .parse::<TimerId>(),
            Ok(id)
        );
        assert!(
            "7".parse::<TimerId>()
                .is_err(),
            "a bare number names no instance"
        );
        assert!(
            "nothex:7"
                .parse::<TimerId>()
                .is_err()
        );
    }

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
    }

    // -- conversions --------------------------------------------------

    #[test]
    fn timer_from_recording_maps_fields() {
        let rec = recording(
            7,
            json!({ "status": "recording",
                    "program": { "title": "MOTD", "description": "Highlights" } }),
        );
        let t = timer_from_recording(ADDON, &rec);
        assert_eq!(t.id, format!("{}:7", ADDON.simple()));
        assert_eq!(t.type_, "Timer");
        assert_eq!(t.name, "MOTD");
        assert_eq!(
            t.overview
                .as_deref(),
            Some("Highlights")
        );
        assert_eq!(t.channel_id, Some(channel_uuid_of(ADDON, 42)));
        assert_eq!((t.start_date, t.end_date), (rec.start_time, rec.end_time));
        assert_eq!(t.status, RecordingStatus::InProgress);
        assert_eq!(t.series_timer_id, None);
        // Only a recording in progress points at its item, by the id its
        // synced Recording row gets.
        assert_eq!(
            t.program_info
                .map(|p| p.id),
            Some(rid(7))
        );

        let unlabelled = timer_from_recording(ADDON, &recording(1, json!({})));
        assert_eq!(unlabelled.name, "Recording");
        assert_eq!(unlabelled.status, RecordingStatus::New);
        assert!(
            unlabelled
                .program_info
                .is_none()
        );
        let v = serde_json::to_value(timer_from_recording(
            ADDON,
            &recording(1, json!({ "status": "completed" })),
        ))
        .unwrap();
        for key in [
            "Id",
            "Type",
            "ServerId",
            "ChannelId",
            "StartDate",
            "EndDate",
            "PrePaddingSeconds",
            "IsPrePaddingRequired",
        ] {
            assert!(!v[key].is_null(), "missing {key} in {v}");
        }
        assert_eq!(v["Status"], "Completed");
        assert!(
            v.get("ProgramInfo")
                .is_none(),
            "omitted when absent: {v}"
        );
    }

    #[test]
    fn series_timer_from_rule_maps_fields() {
        let mut r = rule(Some("BBC1.uk"), Some("MOTD"), Some(3));
        r.mode = "new".into();
        let s = series_timer_from_rule(ADDON, &r);
        assert_eq!(s.id, series_timer_id(ADDON, &r));
        assert_eq!(
            (
                s.type_
                    .as_str(),
                s.name
                    .as_str()
            ),
            ("SeriesTimer", "MOTD")
        );
        assert!(s.record_new_only && s.record_any_time && !s.record_any_channel);

        let any = series_timer_from_rule(ADDON, &rule(None, None, None));
        assert_eq!(any.name, "All programs");
        assert!(any.record_any_channel && !any.record_new_only);
        // The non-nullable properties of Jellyfin's `SeriesTimerInfoDto`.
        let v = serde_json::to_value(any).unwrap();
        for key in [
            "Priority",
            "PostPaddingSeconds",
            "IsPostPaddingRequired",
            "SkipEpisodesInLibrary",
            "KeepUpTo",
        ] {
            assert!(!v[key].is_null(), "{key} missing from {v}");
        }
        assert_eq!(v["KeepUntil"], "UntilDeleted");
        assert_eq!(v["StartDate"], "0001-01-01T00:00:00Z");
        assert_eq!(v["ChannelId"], "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn recording_status_maps_to_jellyfin_states() {
        for (from, want, name) in [
            (Status::Scheduled, RecordingStatus::New, "New"),
            (Status::Recording, RecordingStatus::InProgress, "InProgress"),
            (Status::Completed, RecordingStatus::Completed, "Completed"),
            (Status::Stopped, RecordingStatus::Cancelled, "Cancelled"),
            (Status::Interrupted, RecordingStatus::Cancelled, "Cancelled"),
            (Status::Failed, RecordingStatus::Error, "Error"),
        ] {
            assert_eq!(RecordingStatus::from(from), want, "{from:?}");
            assert_eq!(serde_json::to_value(want).unwrap(), name);
        }
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
                    .as_deref()
            })
            .collect();
        assert_eq!(
            order,
            [
                Some("2026-09-21T20:20:54.518446+00:00"),
                Some("2026-09-12T22:00:00+00:00"),
                Some("2026-09-04T17:33:24.674911+00:00"),
                None,
            ]
        );
    }

    #[test]
    fn an_unreachable_recording_is_in_progress_only_inside_its_window() {
        let now = chrono::Utc::now().naive_utc();
        let at = |start: i64, end: i64| db::Media {
            live_start: Some(now + Duration::minutes(start)),
            live_end: Some(now + Duration::minutes(end)),
            ..Default::default()
        };
        assert_eq!(
            status_from_schedule(&at(-10, 10), now),
            RecordingStatus::InProgress
        );
        assert_eq!(
            status_from_schedule(&at(-20, -10), now),
            RecordingStatus::Completed
        );
        assert_eq!(
            status_from_schedule(&db::Media::default(), now),
            RecordingStatus::Completed
        );
    }

    // -- guide ----------------------------------------------------------

    #[tokio::test]
    async fn a_timer_links_to_its_guide_programme() {
        let env = Env::new().await;
        let db = &env
            .ctx()
            .db;
        let channel = channel_uuid_of(ADDON, 42);
        let other_channel = channel_uuid_of(ADDON, 43);
        let start: DateTime<Utc> = "2026-09-24T07:00:00Z"
            .parse()
            .unwrap();
        let by_start =
            dispatcharr::program_media_id(channel, start, "Gardeners' World");
        let programme =
            |id: Uuid, title: &str, parent: Uuid, key: Option<&str>| db::Media {
                id,
                title: title.into(),
                kind: db::MediaKind::TvProgram,
                parent_id: Some(parent),
                external_ids: db::ExternalIds {
                    dispatcharr_program_id: key.map(str::to_owned),
                    ..Default::default()
                },
                ..Default::default()
            };
        db::Media::upsert(
            db,
            &[db::Media {
                id: other_channel,
                kind: db::MediaKind::TvChannel,
                ..Default::default()
            }],
        )
        .await
        .unwrap();
        db::Media::upsert(
            db,
            &[
                programme(by_start, "Gardeners' World", channel, None),
                programme(
                    Uuid::from_u128(0xa),
                    "South Today",
                    other_channel,
                    Some("276710"),
                ),
                programme(Uuid::from_u128(0xb), "South Today", channel, Some("276710")),
            ],
        )
        .await
        .unwrap();

        let window = json!({
            "start_time": "2026-09-24T07:00:00+00:00",
            "end_time": "2026-09-24T08:00:00+00:00",
        });
        let with_program = |extra: Value| {
            let mut p = window.clone();
            p.as_object_mut()
                .unwrap()
                .extend(
                    extra
                        .as_object()
                        .unwrap()
                        .clone(),
                );
            json!({ "program": p })
        };
        let mut timers: Vec<TimerInfoDto> = [
            recording(7, with_program(json!({ "title": "Gardeners' World" }))),
            recording(8, with_program(json!({ "title": "Not in the guide" }))),
            recording(9, json!({ "title": "Manual" })),
            // What Dispatcharr's own guide stores: the id, but no airing times.
            recording(
                10,
                json!({ "program": { "id": 276710, "title": "South Today" } }),
            ),
        ]
        .iter()
        .map(|r| timer_from_recording(ADDON, r))
        .collect();
        link_programs(db, &mut timers)
            .await
            .unwrap();

        let ids: Vec<_> = timers
            .iter()
            .map(|t| t.program_id)
            .collect();
        assert_eq!(
            ids,
            [Some(by_start), None, None, Some(Uuid::from_u128(0xb))]
        );
        // Serialised the way item ids are, so clients can compare them.
        assert_eq!(
            serde_json::to_value(&timers[0]).unwrap()["ProgramId"],
            json!(
                by_start
                    .simple()
                    .to_string()
            )
        );
    }

    #[tokio::test]
    async fn guide_programmes_carry_the_timer_recording_them() {
        let env = Env::new().await;
        env.json(
            GET,
            "/api/channels/recordings/",
            json!([
                { "id": 7, "channel": 42,
                  "start_time": "2999-01-01T20:00:00Z", "end_time": "2999-01-01T21:00:00Z",
                  "custom_properties": { "program": {
                      "id": 276710, "title": "News", "tvg_id": "BBC1.uk", "epg_source_id": 3,
                  }}},
                { "id": 8, "channel": 42,
                  "start_time": "2000-01-01T20:00:00Z", "end_time": "2000-01-01T21:00:00Z",
                  "custom_properties": { "status": "completed", "program": { "id": 111 } }},
            ]),
        );
        env.json(GET, "/api/channels/series-rules/", news_rules());
        DvrService::invalidate_program_timers();

        let programme = |id: u128, channel: i64, key: &str| db::Media {
            id: Uuid::from_u128(id),
            title: "News".into(),
            kind: db::MediaKind::TvProgram,
            parent_id: Some(channel_uuid_of(ADDON, channel)),
            external_ids: db::ExternalIds {
                iptv_source_id: Some(source(ADDON)),
                dispatcharr_program_id: Some(key.into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let rows = vec![
            programme(1, 42, "276710"),
            programme(2, 42, "111"),
            programme(3, 42, "999"),
            programme(4, 43, "276710"),
        ];
        let timers = DvrService::program_timers(env.ctx(), &rows).await;
        let items: Vec<_> = rows
            .into_iter()
            .map(|r| {
                let item = timers.to_item(r);
                (item.timer_id, item.series_timer_id)
            })
            .collect();

        assert_eq!(
            items,
            [
                (
                    Some(timer(ADDON, 7).to_string()),
                    Some(series_timer_id(
                        ADDON,
                        &rule(Some("BBC1.uk"), Some("News"), Some(3))
                    ))
                ),
                // A finished recording is not a timer.
                (None, None),
                (None, None),
                // Another channel sharing the guide programme isn't recording it.
                (None, None),
            ]
        );
    }

    // -- timers ---------------------------------------------------------

    #[tokio::test]
    async fn a_timer_id_resolves_from_its_number_or_its_recordings_uuid() {
        let env = Env::new().await;
        env.json(GET, "/api/channels/recordings/", json!([]));
        let media = dispatcharr::recording_to_media(
            &recording(7, json!({ "status": "recording" })),
            ADDON,
            &source(ADDON),
        );
        db::Media::upsert(
            &env.ctx()
                .db,
            &[media.clone()],
        )
        .await
        .unwrap();

        let ctx = env.ctx();
        let resolve =
            |id: String| async move { DvrService::resolve_timer_id(ctx, &id).await };
        let want = Some(timer(ADDON, 7));
        assert_eq!(resolve(timer(ADDON, 7).to_string()).await, want);
        assert_eq!(
            resolve(
                media
                    .id
                    .to_string()
            )
            .await,
            want
        );
        assert_eq!(resolve("7".into()).await, None);
        assert_eq!(resolve(Uuid::new_v4().to_string()).await, None);
        assert_eq!(resolve("nope".into()).await, None);
    }

    #[tokio::test]
    async fn dvr_writes_reach_only_the_instance_an_id_names() {
        let env = Env::new().await;
        let other = Uuid::from_u128(0xbbb);
        let removed = Uuid::from_u128(0xdead);
        let other_mock = MockServer::start();
        register_dispatcharr(env.ctx(), other, &other_mock.base_url()).await;
        // Both instances have a recording numbered 7.
        let touched = env
            .mock
            .mock(|when, then| {
                when.any_request();
                then.status(500);
            });
        other_mock.mock(|when, then| {
            when.method(GET)
                .path("/api/channels/recordings/7/");
            then.status(200)
                .json_body(rec_json(7, None));
        });
        let delete_other = other_mock.mock(|when, then| {
            when.method(DELETE)
                .path("/api/channels/recordings/7/");
            then.status(204);
        });

        assert!(
            DvrService::delete_timer(env.ctx(), timer(other, 7))
                .await
                .unwrap()
        );
        delete_other.assert_hits(1);

        for addon in [other, removed] {
            seed_channel(env.ctx(), addon).await;
            let row = dispatcharr::recording_to_media(
                &recording(7, json!({ "status": "completed" })),
                addon,
                &source(addon),
            );
            db::Media::upsert(
                &env.ctx()
                    .db,
                &[row.clone()],
            )
            .await
            .unwrap();
            // The local row goes even when its instance was removed, but no
            // other instance is asked to drop its own recording 7.
            assert!(
                DvrService::delete_recording(env.ctx(), row.id)
                    .await
                    .unwrap()
            );
        }
        delete_other.assert_hits(2);
        touched.assert_hits(0);
        assert!(
            recording_ids(env.ctx())
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_dispatcharr_outage_is_an_error_not_an_empty_answer() {
        let env = Env::new().await;
        env.mock
            .mock(|when, then| {
                when.any_request();
                then.status(500);
            });
        let ctx = env.ctx();

        // Not "the timer is gone", and not "there is nothing scheduled".
        assert!(
            DvrService::get_timer(ctx, timer(ADDON, 7))
                .await
                .is_err()
        );
        assert!(
            DvrService::delete_timer(ctx, timer(ADDON, 7))
                .await
                .is_err()
        );
        assert!(
            DvrService::list_timers(ctx, &TimersFilter::default())
                .await
                .is_err()
        );
        assert!(
            DvrService::list_series_timers(ctx)
                .await
                .is_err()
        );
        assert!(
            DvrService::sync_recordings(ctx, &env.cfg())
                .await
                .is_err()
        );
        // With nothing configured, there is nothing to have failed.
        let (_server, empty) = new_test_server()
            .await
            .unwrap();
        assert!(
            DvrService::list_series_timers(&empty.0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn timers_filter_on_state_and_carry_their_series_timer() {
        let env = Env::new().await;
        env.json(
            GET,
            "/api/channels/recordings/",
            json!([
                { "id": 1, "channel": 42, "start_time": "2999-01-01T20:00:00Z",
                  "end_time": "2999-01-01T21:00:00Z",
                  "custom_properties": { "program": { "title": "News", "tvg_id": "BBC1.uk" } } },
                { "id": 2, "channel": 43, "start_time": "2026-09-15T20:00:00Z",
                  "end_time": "2999-01-01T21:00:00Z",
                  "custom_properties": { "status": "recording" } },
            ]),
        );
        env.json(
            GET,
            "/api/channels/series-rules/",
            json!({ "rules": [{ "tvg_id": "BBC1.uk", "title": "News", "mode": "all" }] }),
        );
        let ctx = env.ctx();
        let ids = |filter: TimersFilter| async move {
            DvrService::list_timers(ctx, &filter)
                .await
                .unwrap()
                .into_iter()
                .map(|t| (t.id, t.series_timer_id))
                .collect::<Vec<_>>()
        };
        let rule_id =
            series_timer_id(ADDON, &rule(Some("BBC1.uk"), Some("News"), None));
        let scheduled = (timer(ADDON, 1).to_string(), Some(rule_id.clone()));
        let running = (timer(ADDON, 2).to_string(), None);

        // Jellyfin Web's Schedule tab.
        assert_eq!(
            ids(TimersFilter {
                is_active: Some(false),
                is_scheduled: Some(true),
                ..Default::default()
            })
            .await,
            [scheduled.clone()]
        );
        assert_eq!(
            ids(TimersFilter {
                is_active: Some(true),
                ..Default::default()
            })
            .await,
            [running.clone()]
        );
        // A series timer's page asks for its own timers, by the id as the
        // client got it back from the path (already decoded).
        assert_eq!(
            ids(TimersFilter {
                series_timer_id: Some(
                    urlencoding::decode(&rule_id)
                        .unwrap()
                        .into_owned()
                ),
                ..Default::default()
            })
            .await,
            [scheduled]
        );
        assert_eq!(
            ids(TimersFilter {
                channel_id: Some(channel_uuid_of(ADDON, 43)),
                ..Default::default()
            })
            .await,
            [running]
        );
        // A single timer names its series timer too.
        env.json(
            GET,
            "/api/channels/recordings/1/",
            json!({ "id": 1, "channel": 42, "start_time": "2999-01-01T20:00:00Z",
                    "end_time": "2999-01-01T21:00:00Z",
                    "custom_properties": { "program": { "title": "News", "tvg_id": "BBC1.uk" } } }),
        );
        assert_eq!(
            DvrService::get_timer(ctx, timer(ADDON, 1))
                .await
                .unwrap()
                .and_then(|t| t.series_timer_id),
            Some(rule_id)
        );
    }

    #[tokio::test]
    async fn a_timer_from_the_guide_sends_its_programme() {
        let env = Env::new().await;
        let at = |h: u32| {
            chrono::NaiveDate::from_ymd_opt(2999, 1, 1)
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap()
        };
        let program = db::Media {
            id: Uuid::from_u128(0x9),
            title: "News".into(),
            kind: db::MediaKind::TvProgram,
            parent_id: Some(channel_uuid_of(ADDON, 42)),
            live_start: Some(at(20)),
            live_end: Some(at(21)),
            external_ids: db::ExternalIds {
                dispatcharr_program_id: Some("276710".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        db::Media::upsert(
            &env.ctx()
                .db,
            &vec![program.clone()],
        )
        .await
        .unwrap();
        env.channels();
        let post = env
            .mock
            .mock(|when, then| {
                when.method(POST)
                    .path("/api/channels/recordings/")
                    .json_body_partial(
                        json!({
                            "channel": 42,
                            "start_time": "2999-01-01T19:58:00+00:00",
                            "end_time": "2999-01-01T21:00:00+00:00",
                            "custom_properties": {
                                "program": {
                                    "id": 276710,
                                    "title": "News",
                                    "start_time": "2999-01-01T20:00:00+00:00",
                                    "end_time": "2999-01-01T21:00:00+00:00",
                                },
                                "remux_padding": { "pre_seconds": 120, "post_seconds": 0 },
                            },
                        })
                        .to_string(),
                    );
                then.status(201)
                    .json_body(rec_json(7, None));
            });

        DvrService::create_timer(
            env.ctx(),
            serde_json::from_value(json!({
                "ProgramId": program.id, "PrePaddingSeconds": 120,
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        post.assert();
    }

    #[tokio::test]
    async fn editing_a_timer_repads_from_the_programme_times() {
        let env = Env::new().await;
        // Already padded by a minute each side once.
        env.json(
            GET,
            "/api/channels/recordings/7/",
            json!({
                "id": 7, "channel": 42,
                "start_time": "2999-01-01T19:59:00Z",
                "end_time": "2999-01-01T21:01:00Z",
                "custom_properties": {
                    "program": {
                        "title": "News",
                        "start_time": "2999-01-01T20:00:00+00:00",
                        "end_time": "2999-01-01T21:00:00+00:00",
                    },
                    "remux_padding": { "pre_seconds": 60, "post_seconds": 60 },
                },
            }),
        );
        let patch = env
            .mock
            .mock(|when, then| {
                when.method(PATCH)
                    .path("/api/channels/recordings/7/")
                    .json_body_partial(
                        json!({
                            "start_time": "2999-01-01T19:55:00+00:00",
                            "end_time": "2999-01-01T21:10:00+00:00",
                            "custom_properties": {
                                "program": { "title": "News" },
                                "remux_padding": { "pre_seconds": 300, "post_seconds": 600 },
                            },
                        })
                        .to_string(),
                    );
                then.status(200)
                    .json_body(json!({
                        "id": 7, "channel": 42,
                        "start_time": "2999-01-01T19:55:00Z",
                        "end_time": "2999-01-01T21:10:00Z",
                    }));
            });
        env.json(
            GET,
            "/api/channels/recordings/8/",
            rec_json(8, Some("recording")),
        );
        let edit = |n: i64| {
            DvrService::update_timer(
                env.ctx(),
                timer(ADDON, n),
                serde_json::from_value(json!({
                    "PrePaddingSeconds": 300, "PostPaddingSeconds": 600,
                }))
                .unwrap(),
            )
        };

        assert_eq!(
            edit(7)
                .await
                .unwrap(),
            TimerUpdate::Updated
        );
        patch.assert();
        assert_eq!(
            edit(8)
                .await
                .unwrap(),
            TimerUpdate::AlreadyStarted
        );
    }

    #[tokio::test]
    async fn cancelling_a_finished_timer_keeps_the_recording() {
        let env = Env::new().await;
        env.json(
            GET,
            "/api/channels/recordings/7/",
            rec_json(7, Some("completed")),
        );
        let delete = env.status(DELETE, "/api/channels/recordings/7/", 204);

        assert!(
            !DvrService::delete_timer(env.ctx(), timer(ADDON, 7))
                .await
                .unwrap()
        );
        assert!(
            DvrService::get_timer(env.ctx(), timer(ADDON, 7))
                .await
                .unwrap()
                .is_none()
        );
        delete.assert_hits(0);
    }

    // -- series timers ----------------------------------------------------

    #[tokio::test]
    async fn a_series_timer_is_pinned_to_its_guide_source_and_evaluated() {
        let env = Env::new().await;
        env.channels();
        env.json(
            GET,
            "/api/epg/epgdata/",
            json!([{ "id": 5, "tvg_id": "BBC1.uk", "epg_source": 3 }]),
        );
        let create = env
            .mock
            .mock(|when, then| {
                when.method(POST)
                    .path("/api/channels/series-rules/")
                    .json_body_partial(
                        json!({ "tvg_id": "BBC1.uk", "title": "News", "epg_source_id": 3 })
                            .to_string(),
                    );
                then.status(200)
                    .json_body(json!({ "success": true, "rules": news_rules()["rules"] }));
            });
        let evaluate = env
            .mock
            .mock(|when, then| {
                when.method(POST)
                    .path("/api/channels/series-rules/evaluate/")
                    .json_body(json!({ "tvg_id": "BBC1.uk" }));
                then.status(200)
                    .json_body(json!({ "success": true }));
            });

        DvrService::create_series_timer(
            env.ctx(),
            serde_json::from_value(json!({
                "ChannelId": channel_uuid_of(ADDON, 42), "Name": "News",
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        create.assert();
        evaluate.assert();
    }

    #[tokio::test]
    async fn a_series_timer_is_found_by_the_id_its_own_listing_returns() {
        // `SeriesTimerInfoDto::id` is percent-encoded, but a path segment
        // reaches the handler already decoded by axum.
        let env = Env::new().await;
        env.json(GET, "/api/channels/series-rules/", news_rules());

        let listed = DvrService::list_series_timers(env.ctx())
            .await
            .unwrap();
        let id = &listed[0].id;
        assert!(id.contains("%7C"), "{id}");
        let found = DvrService::get_series_timer(env.ctx(), id)
            .await
            .unwrap()
            .expect("series timer found by its own listed id");
        assert_eq!(found.id, *id);
        assert!(
            DvrService::get_series_timer(env.ctx(), "nope|nope|nope")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn editing_a_series_timer_changes_only_its_mode() {
        let env = Env::new().await;
        env.json(
            GET,
            "/api/channels/series-rules/",
            json!({ "rules": [{
                "tvg_id": "BBC1.uk", "title": "News", "mode": "all", "epg_source_id": 3,
                "channel_id": 42,
            }]}),
        );
        let save = env
            .mock
            .mock(|when, then| {
                when.method(POST)
                    .path("/api/channels/series-rules/")
                    .json_body_partial(
                        json!({ "tvg_id": "BBC1.uk", "title": "News", "mode": "new", "channel_id": 42 })
                            .to_string(),
                    );
                then.status(200)
                    .json_body(json!({ "success": true, "rules": [] }));
            });
        let evaluate = env.json(
            POST,
            "/api/channels/series-rules/evaluate/",
            json!({ "success": true }),
        );
        let ctx = env.ctx();
        let listed = DvrService::list_series_timers(ctx)
            .await
            .unwrap()
            .remove(0);

        // Posted back as listed: nothing to change.
        let echoed =
            serde_json::from_value(serde_json::to_value(&listed).unwrap()).unwrap();
        assert!(
            DvrService::update_series_timer(ctx, &listed.id, echoed)
                .await
                .unwrap()
        );
        save.assert_hits(0);

        // Fields a Dispatcharr rule doesn't hold are ignored.
        let edit = serde_json::from_value(json!({
            "RecordNewOnly": true, "Name": "Evening News", "PrePaddingSeconds": 60,
        }))
        .unwrap();
        assert!(
            DvrService::update_series_timer(ctx, &listed.id, edit)
                .await
                .unwrap()
        );
        save.assert();
        evaluate.assert();

        let gone =
            series_timer_id(ADDON, &rule(Some("BBC1.uk"), Some("Other"), Some(3)));
        assert!(
            !DvrService::update_series_timer(ctx, &gone, Default::default())
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn deleting_a_series_timer_with_no_rule_is_not_found() {
        let env = Env::new().await;
        env.json(GET, "/api/channels/series-rules/", news_rules());
        let delete = env
            .mock
            .mock(|when, then| {
                when.method(DELETE)
                    .path("/api/channels/series-rules/")
                    .query_param("title", "News");
                then.status(200)
                    .json_body(json!({ "success": true }));
            });

        let id = |title| {
            series_timer_id(ADDON, &rule(Some("BBC1.uk"), Some(title), Some(3)))
        };
        assert!(
            !DvrService::delete_series_timer(env.ctx(), &id("Weather"))
                .await
                .unwrap()
        );
        delete.assert_hits(0);
        assert!(
            DvrService::delete_series_timer(env.ctx(), &id("News"))
                .await
                .unwrap()
        );
        delete.assert();
    }

    // -- recordings -----------------------------------------------------

    #[tokio::test]
    async fn listing_a_running_timer_syncs_the_recording_it_points_at() {
        let env = Env::new().await;
        env.json(
            GET,
            "/api/channels/recordings/",
            json!([rec_json(2, Some("recording")), rec_json(3, None)]),
        );
        assert!(
            recording_ids(env.ctx())
                .await
                .is_empty()
        );

        let timers = DvrService::list_timers(env.ctx(), &TimersFilter::default())
            .await
            .unwrap();
        let running = timers
            .iter()
            .find(|t| t.id == timer(ADDON, 2).to_string())
            .unwrap();
        assert_eq!(
            running
                .program_info
                .as_ref()
                .map(|p| p.id),
            Some(rid(2))
        );
        assert_eq!(recording_ids(env.ctx()).await, [rid(2)]);
    }

    #[tokio::test]
    async fn sync_recordings_skips_scheduled_and_prunes_only_its_own_addon() {
        let env = Env::new().await;
        let ctx = env.ctx();
        let other = Uuid::from_u128(0x0dd);
        seed_channel(ctx, other).await;
        let foreign = dispatcharr::recording_to_media(
            &recording(99, json!({ "status": "completed" })),
            other,
            &source(other),
        );
        db::Media::upsert(&ctx.db, &vec![foreign.clone()])
            .await
            .unwrap();
        let cfg = env.cfg();
        let cfg = &cfg;
        let sync = |recordings: Value| {
            let mut list = env
                .mock
                .mock(|when, then| {
                    when.method(GET)
                        .path("/api/channels/recordings/")
                        .header("X-API-Key", "k");
                    then.status(200)
                        .json_body(recordings);
                });
            async move {
                let n = DvrService::sync_recordings(ctx, cfg)
                    .await
                    .unwrap();
                list.delete();
                n
            }
        };
        let with_foreign = |mut ids: Vec<Uuid>| {
            ids.push(foreign.id);
            ids.sort();
            ids
        };

        // Scheduled (id 1) has no file yet, so it is not synced as playable.
        let all = json!([
            rec_json(1, None),
            rec_json(2, Some("recording")),
            rec_json(3, Some("completed")),
            rec_json(4, Some("failed")),
        ]);
        assert_eq!(sync(all).await, 3);
        assert_eq!(
            recording_ids(ctx).await,
            with_foreign(vec![rid(2), rid(3), rid(4)])
        );
        assert_eq!(
            stream_count(ctx).await,
            3,
            "one playable child per recording"
        );

        // Dropped upstream: pruned with their streams, the other addon's row kept.
        assert_eq!(sync(json!([rec_json(3, Some("completed"))])).await, 1);
        assert_eq!(recording_ids(ctx).await, with_foreign(vec![rid(3)]));
        assert_eq!(stream_count(ctx).await, 1);

        assert_eq!(sync(json!([])).await, 0);
        assert_eq!(recording_ids(ctx).await, with_foreign(vec![]));
    }

    #[tokio::test]
    async fn a_finished_recording_drops_the_probe_of_its_growing_playlist() {
        let env = Env::new().await;
        let ctx = env.ctx();
        let mut list = env.json(
            GET,
            "/api/channels/recordings/",
            json!([rec_json(2, Some("recording"))]),
        );
        DvrService::sync_recordings(ctx, &env.cfg())
            .await
            .unwrap();
        let streams = || async move {
            db::Media::get_by_id(&ctx.db, &rid(2))
                .await
                .unwrap()
                .unwrap()
                .streams(&ctx.db)
                .await
                .unwrap()
        };
        let growing = streams().await;
        assert_eq!(growing.len(), 1);
        db::Media::save_probe_data(
            &ctx.db,
            &growing[0].id,
            &api::MediaSourceInfo::default(),
        )
        .await
        .unwrap();

        list.delete();
        env.json(
            GET,
            "/api/channels/recordings/",
            json!([rec_json(2, Some("completed"))]),
        );
        DvrService::sync_recordings(ctx, &env.cfg())
            .await
            .unwrap();
        let finished = streams().await;
        assert_eq!(finished.len(), 1);
        assert_ne!(finished[0].id, growing[0].id);
        assert!(
            finished[0]
                .probe_data
                .is_none()
        );
        assert_eq!(stream_count(ctx).await, 1);
    }

    #[tokio::test]
    async fn listed_recordings_filter_on_upstream_status_and_channel() {
        let env = Env::new().await;
        // Both are inside the same window, so only the upstream status tells
        // the stopped one apart from one still recording.
        let rec = |id: i64, status: &str| {
            let mut r = rec_json_with(id, 42, json!({ "status": status }));
            r["end_time"] = json!("2999-01-01T00:00:00Z");
            r
        };
        env.json(
            GET,
            "/api/channels/recordings/",
            json!([rec(2, "recording"), rec(3, "stopped")]),
        );
        let ctx = env.ctx();
        let list = |filter: RecordingsFilter| async move {
            DvrService::list_recordings(ctx, &filter)
                .await
                .unwrap()
                .into_iter()
                .map(|i| i.id)
                .collect::<Vec<_>>()
        };
        let in_progress = |v| RecordingsFilter {
            is_in_progress: Some(v),
            ..Default::default()
        };
        let on = |channel| RecordingsFilter {
            channel_id: Some(channel),
            ..Default::default()
        };

        assert_eq!(list(in_progress(true)).await, [rid(2)]);
        assert_eq!(list(in_progress(false)).await, [rid(3)]);
        assert_eq!(
            list(RecordingsFilter {
                status: Some(RecordingStatus::Cancelled),
                ..Default::default()
            })
            .await,
            [rid(3)]
        );
        assert!(
            list(on(Uuid::from_u128(1)))
                .await
                .is_empty()
        );
        assert_eq!(
            list(on(channel_uuid_of(ADDON, 42)))
                .await
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn sync_recordings_imports_the_channels_it_needs_as_parents() {
        // No channel import has run: the state a freshly configured addon is
        // in when a client opens Live TV recordings.
        let env = Env::without_channels().await;
        let mut list = env.json(
            GET,
            "/api/channels/recordings/",
            json!([rec_json(3, Some("completed"))]),
        );
        let channels = env.channels();
        let sync = || async {
            DvrService::sync_recordings(env.ctx(), &env.cfg())
                .await
                .unwrap()
        };

        assert_eq!(sync().await, 1);
        assert_eq!(sync().await, 1);
        channels.assert_hits(1);
        assert_eq!(recording_ids(env.ctx()).await, [rid(3)]);

        // A recording on a channel Dispatcharr no longer lists is skipped
        // without sinking the others.
        list.delete();
        env.json(
            GET,
            "/api/channels/recordings/",
            json!([
                rec_json(3, Some("completed")),
                rec_json_with(4, 77, json!({ "status": "completed" })),
            ]),
        );
        assert_eq!(sync().await, 1);
        assert_eq!(recording_ids(env.ctx()).await, [rid(3)]);
    }

    #[tokio::test]
    async fn concurrent_syncs_for_the_same_addon_are_serialized() {
        let env = Env::new().await;
        env.mock
            .mock(|when, then| {
                when.method(GET)
                    .path("/api/channels/recordings/");
                then.status(200)
                    .delay(std::time::Duration::from_millis(250))
                    .json_body(json!([rec_json(3, Some("completed"))]));
            });
        let cfg = env.cfg();

        let started = std::time::Instant::now();
        let (a, b) = tokio::join!(
            DvrService::sync_recordings(env.ctx(), &cfg),
            DvrService::sync_recordings(env.ctx(), &cfg),
        );
        a.unwrap();
        b.unwrap();
        // Unserialized, both 250ms fetches overlap and the pair takes ~250ms.
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(450),
            "{:?}",
            started.elapsed()
        );
    }
}
