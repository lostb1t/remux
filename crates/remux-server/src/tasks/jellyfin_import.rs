use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::NaiveDate;
use std::{collections::HashMap, sync::Arc};
use tracing::{debug, info, warn};

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{AppContext, db};
use remux_sdks::{
    JellyfinApiKeyAuth, RestClient,
    remux::{
        GetJellyfinItemsByIds, GetJellyfinUserItems, GetJellyfinUsers, JellyfinItem,
        JellyfinUserDto,
    },
};

pub struct JellyfinImportTask;

#[async_trait]
impl Task for JellyfinImportTask {
    fn key(&self) -> &str {
        "JellyfinImport"
    }

    fn name(&self) -> &str {
        "Import from Jellyfin"
    }

    fn description(&self) -> &str {
        "Imports users and watch history from Jellyfin."
    }
    fn short_description(&self) -> &str {
        "Imports users and watch history from Jellyfin"
    }
    fn category(&self) -> TaskCategory {
        TaskCategory::Users
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let config = db::Settings::get_config(&ctx.db).await?;
        let url = config
            .jellyfin_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("Jellyfin URL is not configured"))?
            .to_string();
        let api_key = config
            .jellyfin_api_key
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("Jellyfin API key is not configured"))?
            .to_string();

        let client = RestClient::new(&url)?.with_auth(JellyfinApiKeyAuth { api_key });

        info!("fetching user list from {url}");
        let jf_users = client
            .execute(GetJellyfinUsers)
            .await?;
        info!("building media index");
        let index = build_media_index(&ctx.db).await?;
        info!(
            imdb = index
                .by_imdb
                .len(),
            tmdb = index
                .by_tmdb
                .len(),
            tvdb = index
                .by_tvdb
                .len(),
            "media index built"
        );

        info!("syncing {} Jellyfin users", jf_users.len());
        progress.set(5.0);

        // Create/find local users
        let mut local_users: Vec<(JellyfinUserDto, db::User)> = Vec::new();
        let mut users_created = 0u32;
        for jf_user in jf_users {
            let Some(username) = jf_user
                .name
                .as_deref()
            else {
                continue;
            };
            let is_admin = jf_user
                .policy
                .as_ref()
                .and_then(|p| p.is_administrator)
                .unwrap_or(false);

            let local_user = match db::User::get_by_username(&ctx.db, username).await? {
                Some(u) => {
                    debug!("skipping existing user '{username}'");
                    u
                }
                None => {
                    let random_pw = uuid::Uuid::new_v4().to_string();
                    let mut user = db::User::new_with_password(
                        String::new(),
                        username.to_string(),
                        &random_pw,
                        None,
                    )?;
                    user.is_admin = is_admin;
                    user.save(&ctx.db)
                        .await?;
                    debug!("created user '{username}'");
                    users_created += 1;
                    user
                }
            };
            local_users.push((jf_user, local_user));
        }
        progress.set(10.0);

        // Pass 1: fetch all user items, collect unique series IDs needed for episode resolution
        let mut user_items: Vec<(
            usize,
            &JellyfinUserDto,
            &db::User,
            Vec<JellyfinItem>,
        )> = Vec::new();
        let mut needed_series_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for (i, (jf_user, local_user)) in local_users
            .iter()
            .enumerate()
        {
            let Some(jf_id) = jf_user
                .id
                .as_deref()
            else {
                continue;
            };
            let username = jf_user
                .name
                .as_deref()
                .unwrap_or("?");
            debug!(
                "fetching items for user '{username}' ({}/{})",
                i + 1,
                local_users.len()
            );
            let mut seen = std::collections::HashSet::new();
            let mut items: Vec<JellyfinItem> = Vec::new();
            for filter in &["IsPlayed", "IsResumable", "IsFavorite"] {
                const PAGE: i32 = 100;
                let mut start = 0i32;
                loop {
                    let page = client
                        .execute(GetJellyfinUserItems {
                            user_id: jf_id.to_string(),
                            filter,
                            start_index: start,
                            limit: PAGE,
                        })
                        .await?;
                    let fetched = page
                        .items
                        .len() as i32;
                    let total = page.total_record_count as i32;
                    for it in page.items {
                        if seen.insert(
                            it.id
                                .clone(),
                        ) {
                            items.push(it);
                        }
                    }
                    start += fetched;
                    if start >= total || fetched < PAGE {
                        break;
                    }
                }
            }
            debug!("got {} items for '{username}'", items.len());

            for item in &items {
                if matches!(
                    item.item_type
                        .as_deref(),
                    Some("Episode") | Some("Season")
                ) {
                    // Only need series lookup when SeriesProviderIds didn't give us IMDB
                    let has_series_imdb = item
                        .series_provider_ids
                        .as_ref()
                        .and_then(|p| p.get("Imdb"))
                        .is_some();
                    if !has_series_imdb {
                        if let Some(sid) = &item.series_id {
                            needed_series_ids.insert(sid.clone());
                        }
                    }
                }
            }

            user_items.push((i, jf_user, local_user, items));
        }
        progress.set(50.0);

        // Pass 2: batch-fetch only the series we actually need
        debug!(
            count = needed_series_ids.len(),
            "fetching series provider IDs for episode resolution"
        );
        let series_imdb_map: HashMap<String, String> = if needed_series_ids.is_empty() {
            HashMap::new()
        } else {
            let ids = needed_series_ids
                .into_iter()
                .collect::<Vec<_>>();
            client
                .execute(GetJellyfinItemsByIds { ids })
                .await?
                .items
                .into_iter()
                .filter_map(|it| {
                    let id = it.id?;
                    let imdb = it
                        .provider_ids?
                        .get("Imdb")?
                        .clone();
                    Some((id, imdb))
                })
                .collect()
        };
        debug!(count = series_imdb_map.len(), "series index built");
        progress.set(60.0);

        // Pass 2b: seed media stubs for items not yet in the local DB.
        // Collect unique top-level items (Movie or Series) across all users, then
        // run process_meta_batch so they get full metadata + child tree immediately.
        {
            let mut stubs: Vec<db::Media> = Vec::new();
            let mut seen_stubs: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for (_, _, _, items) in &user_items {
                for item in items {
                    let provider_ids = item
                        .provider_ids
                        .as_ref();
                    let imdb = provider_ids
                        .and_then(|p| p.get("Imdb"))
                        .map(String::as_str);
                    let tmdb = provider_ids
                        .and_then(|p| p.get("Tmdb"))
                        .and_then(|v| {
                            v.parse::<i64>()
                                .ok()
                        });
                    let tvdb = provider_ids
                        .and_then(|p| p.get("Tvdb"))
                        .and_then(|v| {
                            v.parse::<i64>()
                                .ok()
                        });

                    // For episodes/seasons, derive the parent series' external IDs
                    let (top_kind, top_imdb, top_tmdb, top_tvdb) = match item
                        .item_type
                        .as_deref()
                    {
                        Some("Movie") => {
                            (db::MediaKind::Movie, imdb.map(String::from), tmdb, tvdb)
                        }
                        Some("Series") => {
                            (db::MediaKind::Series, imdb.map(String::from), tmdb, tvdb)
                        }
                        Some("Episode") | Some("Season") => {
                            let sp = item
                                .series_provider_ids
                                .as_ref();
                            let s_imdb = sp
                                .and_then(|p| p.get("Imdb"))
                                .map(String::from)
                                .or_else(|| {
                                    item.series_id
                                        .as_deref()
                                        .and_then(|sid| series_imdb_map.get(sid))
                                        .cloned()
                                });
                            let s_tmdb = sp
                                .and_then(|p| p.get("Tmdb"))
                                .and_then(|v| {
                                    v.parse::<i64>()
                                        .ok()
                                });
                            let s_tvdb = sp
                                .and_then(|p| p.get("Tvdb"))
                                .and_then(|v| {
                                    v.parse::<i64>()
                                        .ok()
                                });
                            (db::MediaKind::Series, s_imdb, s_tmdb, s_tvdb)
                        }
                        _ => continue,
                    };

                    // Nothing to key on → skip
                    if top_imdb.is_none() && top_tmdb.is_none() && top_tvdb.is_none() {
                        continue;
                    }

                    let ext = db::ExternalIds {
                        imdb: top_imdb
                            .as_deref()
                            .and_then(|s| {
                                db::NonEmptyString::try_new(s.to_string()).ok()
                            }),
                        tmdb: top_tmdb,
                        tvdb: top_tvdb,
                        ..Default::default()
                    };

                    // Build a stable dedup key from external IDs so the same media
                    // is not queued twice even though stubs now use random UUIDs.
                    let dedup_key = format!(
                        "{:?}:imdb={:?}:tmdb={:?}:tvdb={:?}",
                        top_kind,
                        top_imdb.as_deref(),
                        top_tmdb,
                        top_tvdb
                    );

                    // Already in local DB or already queued → skip
                    if resolve_from_index(
                        &index,
                        top_imdb.as_deref(),
                        top_tmdb,
                        top_tvdb,
                    )
                    .is_some()
                        || seen_stubs.contains(&dedup_key)
                    {
                        continue;
                    }

                    // For Series/Movie items we have the title directly;
                    // for derived series stubs (from episodes) we may not.
                    let title = match item
                        .item_type
                        .as_deref()
                    {
                        Some("Movie") | Some("Series") => item
                            .name
                            .clone()
                            .unwrap_or_default(),
                        _ => String::new(), // refresh_meta will fill this in
                    };
                    if title.is_empty() && top_imdb.is_none() && top_tmdb.is_none() {
                        continue;
                    }

                    let released_at = item
                        .production_year
                        .and_then(|y| {
                            NaiveDate::from_ymd_opt(y as i32, 1, 1)
                                .and_then(|d| d.and_hms_opt(0, 0, 0))
                        });
                    let runtime = item
                        .run_time_ticks
                        .map(|t| t / 10_000_000);

                    let stub = db::Media {
                        id: uuid::Uuid::new_v4(),
                        kind: top_kind,
                        title,
                        external_ids: ext,
                        description: item
                            .overview
                            .clone(),
                        released_at,
                        runtime,
                        ..Default::default()
                    };
                    seen_stubs.insert(dedup_key);
                    stubs.push(stub);
                }
            }

            if !stubs.is_empty() {
                debug!(
                    count = stubs.len(),
                    "seeding missing media stubs from Jellyfin"
                );
                ctx.addons
                    .process_meta_batch(stubs, &ctx, false, None)
                    .await?;
            }
        }
        progress.set(70.0);

        // Pass 3: import watch states
        let mut states_imported = 0u32;
        let mut states_unresolved = 0u32;
        let user_count = user_items.len();

        for (i, jf_user, local_user, items) in user_items {
            let username = jf_user
                .name
                .as_deref()
                .unwrap_or("?");
            debug!("importing {} items for '{username}'", items.len());

            for item in items {
                let Some(ud) = &item.user_data else {
                    continue;
                };
                let play_count = ud
                    .play_count
                    .unwrap_or(0);
                let position = ud
                    .playback_position_ticks
                    .unwrap_or(0);
                let favorite = ud
                    .is_favorite
                    .unwrap_or(false);

                if play_count == 0 && position == 0 && !favorite {
                    continue;
                }

                let provider_ids = item
                    .provider_ids
                    .as_ref();
                let imdb = provider_ids
                    .and_then(|p| p.get("Imdb"))
                    .map(String::as_str);
                let tmdb = provider_ids
                    .and_then(|p| p.get("Tmdb"))
                    .and_then(|v| {
                        v.parse::<i64>()
                            .ok()
                    });
                let tvdb = provider_ids
                    .and_then(|p| p.get("Tvdb"))
                    .and_then(|v| {
                        v.parse::<i64>()
                            .ok()
                    });

                let kind = match item
                    .item_type
                    .as_deref()
                {
                    Some("Movie") => db::MediaKind::Movie,
                    Some("Series") => db::MediaKind::Series,
                    Some("Season") => db::MediaKind::Season,
                    Some("Episode") => db::MediaKind::Episode,
                    _ => db::MediaKind::Movie,
                };
                // For Season/Episode: check if we resolved a series IMDB (used for has_ids).
                // ProviderIds["Imdb"] is NOT used — it can be the episode's own IMDB.
                let series_imdb_resolved =
                    matches!(kind, db::MediaKind::Season | db::MediaKind::Episode)
                        && (item
                            .series_provider_ids
                            .as_ref()
                            .and_then(|p| p.get("Imdb"))
                            .is_some()
                            || item
                                .series_id
                                .as_deref()
                                .and_then(|sid| series_imdb_map.get(sid))
                                .is_some());

                let raw = db::MediaIdRaw {
                    kind: kind.clone(),
                    external_ids: db::ExternalIds {
                        imdb: matches!(
                            kind,
                            db::MediaKind::Movie | db::MediaKind::Series
                        )
                        .then(|| {
                            imdb.and_then(|s| {
                                db::NonEmptyString::try_new(s.to_string()).ok()
                            })
                        })
                        .flatten(),
                        tmdb,
                        tvdb,
                        ..Default::default()
                    },
                    season: item.parent_index_number,
                    episode: item.index_number,
                };

                let has_ids = raw
                    .external_ids
                    .imdb
                    .is_some()
                    || series_imdb_resolved
                    || raw
                        .external_ids
                        .tmdb
                        .is_some()
                    || raw
                        .external_ids
                        .tvdb
                        .is_some();
                if !has_ids {
                    warn!(
                        name = item
                            .name
                            .as_deref()
                            .unwrap_or("?"),
                        item_type = item
                            .item_type
                            .as_deref(),
                        "no external IDs, skipping"
                    );
                    states_unresolved += 1;
                    continue;
                }

                // Use the local DB UUID when the item is already imported; otherwise
                // derive the stable UUID from external IDs so get_or_new can find the
                // state row via ext_id_uuid_candidates when the item is imported later.
                //
                // For episodes/seasons: ID-based lookup rarely matches (Remux stores
                // stremio/tmdb, Jellyfin stores imdb/tvdb). Fall back to positional
                // lookup via (grandparent_series_uuid, season, episode) as designed
                // in find_existing_id_by_ext / ext_id_uuid_candidates.
                let media_uuid = if let Some(uuid) =
                    resolve_from_index(&index, imdb, tmdb, tvdb)
                {
                    uuid
                } else if matches!(kind, db::MediaKind::Episode | db::MediaKind::Season)
                {
                    let sp = item
                        .series_provider_ids
                        .as_ref();
                    let series_imdb = sp
                        .and_then(|p| p.get("Imdb"))
                        .map(String::as_str)
                        .or_else(|| {
                            item.series_id
                                .as_deref()
                                .and_then(|sid| series_imdb_map.get(sid))
                                .map(String::as_str)
                        });
                    let series_tmdb = sp
                        .and_then(|p| p.get("Tmdb"))
                        .and_then(|v| {
                            v.parse::<i64>()
                                .ok()
                        });
                    let series_tvdb = sp
                        .and_then(|p| p.get("Tvdb"))
                        .and_then(|v| {
                            v.parse::<i64>()
                                .ok()
                        });
                    let positional = if let Some(series_uuid) = resolve_from_index(
                        &index,
                        series_imdb,
                        series_tmdb,
                        series_tvdb,
                    ) {
                        if let (Some(season_num), Some(ep_num)) =
                            (item.parent_index_number, item.index_number)
                        {
                            sqlx::query_scalar(
                                "SELECT id FROM media WHERE kind = 'episode' \
                                 AND grandparent_id = ? AND parent_idx = ? AND idx = ? LIMIT 1",
                            )
                            .bind(series_uuid)
                            .bind(season_num as i64)
                            .bind(ep_num as i64)
                            .fetch_optional(&ctx.db)
                            .await
                            .ok()
                            .flatten()
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    positional.unwrap_or_else(|| uuid::Uuid::from(&raw))
                } else {
                    uuid::Uuid::from(&raw)
                };

                let played_at = ud
                    .last_played_date
                    .map(|dt| dt.naive_utc());
                let last_played_at = played_at;
                let state = db::UserMediaState {
                    user_id: local_user.id,
                    media_id: media_uuid,
                    media_raw: serde_json::to_string(&raw).ok(),
                    favorite,
                    play_count,
                    played_at,
                    playback_position: position / 10_000_000,
                    ..Default::default()
                };
                sqlx::query(
                    "INSERT INTO user_media_state \
                     (user_id, media_id, media_raw, favorite, play_count, played_at, \
                      playback_position, last_played_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT(user_id, media_id) DO UPDATE SET \
                       media_raw = excluded.media_raw, \
                       favorite = excluded.favorite, \
                       play_count = CASE \
                         WHEN excluded.last_played_at > user_media_state.last_played_at \
                           OR user_media_state.last_played_at IS NULL \
                         THEN excluded.play_count \
                         ELSE user_media_state.play_count END, \
                       played_at = CASE \
                         WHEN excluded.last_played_at > user_media_state.last_played_at \
                           OR user_media_state.last_played_at IS NULL \
                         THEN excluded.played_at \
                         ELSE user_media_state.played_at END, \
                       playback_position = CASE \
                         WHEN excluded.last_played_at > user_media_state.last_played_at \
                           OR user_media_state.last_played_at IS NULL \
                         THEN excluded.playback_position \
                         ELSE user_media_state.playback_position END, \
                       last_played_at = CASE \
                         WHEN excluded.last_played_at > user_media_state.last_played_at \
                           OR user_media_state.last_played_at IS NULL \
                         THEN excluded.last_played_at \
                         ELSE user_media_state.last_played_at END",
                )
                .bind(state.user_id)
                .bind(state.media_id)
                .bind(&state.media_raw)
                .bind(state.favorite)
                .bind(state.play_count)
                .bind(state.played_at)
                .bind(state.playback_position)
                .bind(last_played_at)
                .execute(&ctx.db)
                .await?;
                states_imported += 1;
            }

            progress.report(i + 1, user_count);
        }

        progress.set(100.0);
        info!(
            users_created,
            states_imported, states_unresolved, "Jellyfin import complete"
        );
        Ok(())
    }
}

struct MediaIndex {
    by_imdb: HashMap<String, uuid::Uuid>,
    by_tmdb: HashMap<i64, uuid::Uuid>,
    by_tvdb: HashMap<i64, uuid::Uuid>,
}

async fn build_media_index(db: &sqlx::SqlitePool) -> Result<MediaIndex> {
    use sqlx::Row as _;
    let rows = sqlx::query(
        "SELECT id, json_extract(external_ids, '$.imdb') as imdb, \
         json_extract(external_ids, '$.tmdb') as tmdb, \
         json_extract(external_ids, '$.tvdb') as tvdb \
         FROM media WHERE external_ids IS NOT NULL AND external_ids != '{}'",
    )
    .fetch_all(db)
    .await?;

    let mut index = MediaIndex {
        by_imdb: HashMap::new(),
        by_tmdb: HashMap::new(),
        by_tvdb: HashMap::new(),
    };

    for row in rows {
        let id: uuid::Uuid = row
            .try_get("id")
            .ok()
            .flatten()
            .unwrap_or_default();
        let imdb: Option<String> = row
            .try_get("imdb")
            .ok()
            .flatten();
        let tmdb: Option<i64> = row
            .try_get("tmdb")
            .ok()
            .flatten();
        let tvdb: Option<i64> = row
            .try_get("tvdb")
            .ok()
            .flatten();

        if imdb.is_none() && tmdb.is_none() && tvdb.is_none() {
            continue;
        }
        if let Some(imdb_id) = imdb {
            index
                .by_imdb
                .insert(imdb_id, id);
        }
        if let Some(tmdb_id) = tmdb {
            index
                .by_tmdb
                .insert(tmdb_id, id);
        }
        if let Some(tvdb_id) = tvdb {
            index
                .by_tvdb
                .insert(tvdb_id, id);
        }
    }

    Ok(index)
}

fn resolve_from_index(
    index: &MediaIndex,
    imdb: Option<&str>,
    tmdb: Option<i64>,
    tvdb: Option<i64>,
) -> Option<uuid::Uuid> {
    if let Some(id) = imdb {
        if let Some(&uuid) = index
            .by_imdb
            .get(id)
        {
            return Some(uuid);
        }
    }
    if let Some(id) = tmdb {
        if let Some(&uuid) = index
            .by_tmdb
            .get(&id)
        {
            return Some(uuid);
        }
    }
    if let Some(id) = tvdb {
        if let Some(&uuid) = index
            .by_tvdb
            .get(&id)
        {
            return Some(uuid);
        }
    }
    None
}
