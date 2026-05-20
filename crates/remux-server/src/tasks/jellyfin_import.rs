use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::{ProgressReporter, Task, TaskService};
use crate::{AppContext, db};
use remux_sdks::remux::{GetJellyfinUserItems, GetJellyfinUsers, JellyfinUserDto};
use remux_sdks::{JellyfinApiKeyAuth, RestClient};

pub struct JellyfinImportTask;

#[async_trait]
impl Task for JellyfinImportTask {
    fn key(&self) -> &str {
        "JellyfinImport"
    }

    fn name(&self) -> &str {
        "Import user history"
    }

    fn description(&self) -> &str {
        "Imports the media library from a connected Jellyfin server."
    }

    fn category(&self) -> &str {
        "Import"
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
        let jf_users = client.execute(GetJellyfinUsers).await?;
        info!("building media index");
        let index = build_media_index(&ctx.db).await?;
        info!(
            imdb = index.by_imdb.len(),
            tmdb = index.by_tmdb.len(),
            tvdb = index.by_tvdb.len(),
            "media index built"
        );
        info!("syncing {} Jellyfin users", jf_users.len());
        progress.set(5.0);

        // Create/find local users
        let mut local_users: Vec<(JellyfinUserDto, db::User)> = Vec::new();
        let mut users_created = 0u32;
        for jf_user in jf_users {
            let Some(username) = jf_user.name.as_deref() else {
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
                    user.save(&ctx.db).await?;
                    debug!("created user '{username}'");
                    users_created += 1;
                    user
                }
            };
            local_users.push((jf_user, local_user));
        }
        progress.set(20.0);

        // Import watch states per user sequentially
        let mut states_imported = 0u32;
        let mut states_unresolved = 0u32;

        for (i, (jf_user, local_user)) in local_users.iter().enumerate() {
            let Some(jf_id) = jf_user.id.as_deref() else {
                continue;
            };
            let username = jf_user.name.as_deref().unwrap_or("?");
            info!(
                "fetching items for user '{username}' ({}/{})",
                i + 1,
                local_users.len()
            );
            let (played, resumable, favorited) = tokio::join!(
                client.execute(GetJellyfinUserItems {
                    user_id: jf_id.to_string(),
                    filter: "IsPlayed"
                }),
                client.execute(GetJellyfinUserItems {
                    user_id: jf_id.to_string(),
                    filter: "IsResumable"
                }),
                client.execute(GetJellyfinUserItems {
                    user_id: jf_id.to_string(),
                    filter: "IsFavorite"
                }),
            );
            let mut seen = std::collections::HashSet::new();
            let mut items: Vec<_> = played?
                .items
                .into_iter()
                .chain(resumable?.items)
                .chain(favorited?.items)
                .filter(|it| seen.insert(it.id.clone()))
                .collect();
            info!("got {} items for '{username}', importing", items.len());

            for item in items {
                let Some(ud) = &item.user_data else {
                    continue;
                };
                let play_count = ud.play_count.unwrap_or(0);
                let position = ud.playback_position_ticks.unwrap_or(0);
                let favorite = ud.is_favorite.unwrap_or(false);

                if play_count == 0 && position == 0 && !favorite {
                    continue;
                }

                let provider_ids = item.provider_ids.as_ref();
                let imdb = provider_ids.and_then(|p| p.get("Imdb")).map(String::as_str);
                let tmdb = provider_ids
                    .and_then(|p| p.get("Tmdb"))
                    .and_then(|v| v.parse::<i64>().ok());
                let tvdb = provider_ids
                    .and_then(|p| p.get("Tvdb"))
                    .and_then(|v| v.parse::<i64>().ok());

                let Some(media_key) = resolve_from_index(&index, imdb, tmdb, tvdb)
                else {
                    let item_type = item.item_type.as_deref();
                    let season = item.parent_index_number;
                    let episode = item.index_number;
                    warn!(
                        name = item.name.as_deref().unwrap_or("?"),
                        item_type,
                        imdb,
                        tmdb,
                        tvdb,
                        season,
                        episode,
                        "could not resolve item to local media"
                    );
                    states_unresolved += 1;
                    continue;
                };

                let state = db::UserMediaState {
                    user_id: local_user.id,
                    media_key,
                    favorite,
                    play_count,
                    played_at: ud.last_played_date.map(|dt| dt.naive_utc()),
                    playback_position: position / 10_000_000,
                    ..Default::default()
                };
                state.save(&ctx.db).await?;
                states_imported += 1;
            }

            progress.report(i + 1, local_users.len());
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
    by_imdb: HashMap<String, String>,
    by_tmdb: HashMap<i64, String>,
    by_tvdb: HashMap<i64, String>,
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
        let id: uuid::Uuid = row.try_get("id").ok().flatten().unwrap_or_default();
        let key = id.as_simple().to_string();
        let imdb: Option<String> = row.try_get("imdb").ok().flatten();
        let tmdb: Option<i64> = row.try_get("tmdb").ok().flatten();
        let tvdb: Option<i64> = row.try_get("tvdb").ok().flatten();

        if imdb.is_none() && tmdb.is_none() && tvdb.is_none() {
            continue;
        }
        if let Some(id) = imdb {
            index.by_imdb.insert(id, key.clone());
        }
        if let Some(id) = tmdb {
            index.by_tmdb.insert(id, key.clone());
        }
        if let Some(id) = tvdb {
            index.by_tvdb.insert(id, key.clone());
        }
    }

    Ok(index)
}

fn resolve_from_index(
    index: &MediaIndex,
    imdb: Option<&str>,
    tmdb: Option<i64>,
    tvdb: Option<i64>,
) -> Option<String> {
    if let Some(id) = imdb {
        if let Some(key) = index.by_imdb.get(id) {
            return Some(key.clone());
        }
    }
    if let Some(id) = tmdb {
        if let Some(key) = index.by_tmdb.get(&id) {
            return Some(key.clone());
        }
    }
    if let Some(id) = tvdb {
        if let Some(key) = index.by_tvdb.get(&id) {
            return Some(key.clone());
        }
    }
    None
}
