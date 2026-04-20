use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::future::try_join_all;
use std::sync::Arc;
use tracing::{debug, info};

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
        "Import from Jellyfin"
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

        let jf_users = client.execute(GetJellyfinUsers).await?;
        info!("fetched {} Jellyfin users", jf_users.len());
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

        // Fetch all users' items in parallel
        let fetches: Vec<_> = local_users
            .iter()
            .filter_map(|(jf_user, _)| jf_user.id.clone())
            .map(|jf_id| {
                let c = client.clone();
                async move {
                    c.execute(GetJellyfinUserItems { user_id: jf_id }).await
                }
            })
            .collect();

        let all_items = try_join_all(fetches).await?;
        progress.set(80.0);

        // Import watch states
        let mut states_imported = 0u32;
        let mut states_unresolved = 0u32;

        for ((_, local_user), result) in local_users.iter().zip(all_items) {
            for item in result.items {
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
                let imdb = provider_ids
                    .and_then(|p| p.get("Imdb"))
                    .map(String::as_str);
                let tmdb = provider_ids
                    .and_then(|p| p.get("Tmdb"))
                    .and_then(|v| v.parse::<i64>().ok());
                let tvdb = provider_ids
                    .and_then(|p| p.get("Tvdb"))
                    .and_then(|v| v.parse::<i64>().ok());

                let Some(media_key) = resolve_media_key(&ctx.db, imdb, tmdb, tvdb).await? else {
                    states_unresolved += 1;
                    continue;
                };

                let state = db::UserMediaState {
                    user_id: local_user.id,
                    media_key,
                    favorite,
                    play_count,
                    played_at: ud.last_played_date.map(|dt| dt.naive_utc()),
                    playback_position: position,
                    ..Default::default()
                };
                state.save(&ctx.db).await?;
                states_imported += 1;
            }
        }

        progress.set(100.0);
        info!(
            users_created,
            states_imported,
            states_unresolved,
            "Jellyfin import complete"
        );
        Ok(())
    }
}

async fn resolve_media_key(
    db: &sqlx::SqlitePool,
    imdb: Option<&str>,
    tmdb: Option<i64>,
    tvdb: Option<i64>,
) -> Result<Option<String>> {
    if let Some(id) = imdb {
        let row = sqlx::query(
            "SELECT media_id FROM media WHERE json_extract(external_ids, '$.imdb') = ? LIMIT 1",
        )
        .bind(id)
        .fetch_optional(db)
        .await?;
        if let Some(row) = row {
            use sqlx::Row as _;
            let media_id: Option<String> = row.try_get("media_id").ok().flatten();
            return Ok(Some(media_id.unwrap_or_else(|| id.to_string())));
        }
    }

    if let Some(id) = tmdb {
        let row = sqlx::query(
            "SELECT media_id FROM media WHERE json_extract(external_ids, '$.tmdb') = ? LIMIT 1",
        )
        .bind(id)
        .fetch_optional(db)
        .await?;
        if let Some(row) = row {
            use sqlx::Row as _;
            let media_id: Option<String> = row.try_get("media_id").ok().flatten();
            if let Some(key) = media_id {
                return Ok(Some(key));
            }
        }
    }

    if let Some(id) = tvdb {
        let row = sqlx::query(
            "SELECT media_id FROM media WHERE json_extract(external_ids, '$.tvdb') = ? LIMIT 1",
        )
        .bind(id)
        .fetch_optional(db)
        .await?;
        if let Some(row) = row {
            use sqlx::Row as _;
            let media_id: Option<String> = row.try_get("media_id").ok().flatten();
            if let Some(key) = media_id {
                return Ok(Some(key));
            }
        }
    }

    Ok(None)
}
