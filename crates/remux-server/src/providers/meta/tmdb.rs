use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tracing::warn;

use crate::sdks::CachedEndpoint;
use crate::{AppContext, db, sdks};

use super::{MetaProvider, MetaResult};

pub struct TmdbMetaProvider;

const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";

fn tmdb_image(path: Option<&str>) -> Option<String> {
    path.filter(|p| !p.is_empty())
        .map(|p| format!("{}{}", TMDB_IMAGE_BASE, p))
}

#[async_trait]
impl MetaProvider for TmdbMetaProvider {
    async fn fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<MetaResult>> {
        // 1. Read API key from settings — skip if not configured
        let config = crate::db::Settings::get_config(&ctx.db).await?;
        let api_key = match config.tmdb_api_key.as_deref().filter(|k| !k.is_empty()) {
            Some(k) => k.to_string(),
            None => return Ok(None),
        };

        // 2. Determine the best available external ID and its source type.
        //    Priority: tmdb > imdb > tvdb.
        //    For seasons/episodes fall back to series_media_id which holds the series imdb.
        let ids = &media.external_ids;
        let lookup = if let Some(tmdb_id) = ids.tmdb {
            Some((tmdb_id.to_string(), "tmdb_id"))
        } else if let Some(ref imdb) = ids.imdb {
            Some((imdb.clone(), "imdb_id"))
        } else if let Some(ref series_id) = media.series_media_id {
            // series_media_id is the parent series' aio_id; for series that is the imdb id
            Some((series_id.clone(), "imdb_id"))
        } else if let Some(tvdb_id) = ids.tvdb {
            Some((tvdb_id.to_string(), "tvdb_id"))
        } else {
            None
        };

        let (external_id, external_source) = match lookup {
            Some(pair) => pair,
            None => return Ok(None),
        };

        // 3. TMDB Find-by-external-ID
        let client = sdks::RestClient::new("https://api.themoviedb.org/3/")?
            .with_auth(sdks::BearerAuth { token: api_key });

        let find_resp = client
            .execute(
                sdks::tmdb::FindByIdEndpoint {
                    external_id,
                    external_source: external_source.to_string(),
                }
                .with_cache(Duration::from_secs(3600)),
            )
            .await;

        let find_resp = match find_resp {
            Ok(r) => r,
            Err(e) => {
                warn!("tmdb find error: {e}");
                return Ok(None);
            }
        };

        // 4. Map TMDB result → MetaResult, storing TMDB id (and imdb if available) back.
        let result_media = match media.kind {
            db::MediaKind::Movie => {
                find_resp.movie_results.into_iter().next().map(|m| {
                    let external_ids = db::ExternalIds {
                        tmdb: Some(m.id),
                        imdb: m.imdb_id.clone().or(ids.imdb.clone()),
                        tvdb: ids.tvdb,
                    };
                    db::Media {
                        title: m.title,
                        description: m.overview,
                        released_at: m.release_date.map(|d| d.and_hms_opt(0, 0, 0).unwrap()),
                        runtime: m.runtime.map(|r| r * 60), // minutes → seconds
                        rating_audience: m.vote_average,
                        poster: tmdb_image(m.poster_path.as_deref()),
                        backdrop: tmdb_image(m.backdrop_path.as_deref()),
                        external_ids: sqlx::types::Json(external_ids),
                        ..Default::default()
                    }
                })
            }
            db::MediaKind::Series => {
                find_resp.tv_results.into_iter().next().map(|s| {
                    let external_ids = db::ExternalIds {
                        tmdb: Some(s.id),
                        imdb: ids.imdb.clone(),
                        tvdb: ids.tvdb,
                    };
                    db::Media {
                        title: s.name,
                        description: s.overview,
                        released_at: s.first_air_date.map(|d| d.and_hms_opt(0, 0, 0).unwrap()),
                        rating_audience: s.vote_average,
                        poster: tmdb_image(s.poster_path.as_deref()),
                        backdrop: tmdb_image(s.backdrop_path.as_deref()),
                        external_ids: sqlx::types::Json(external_ids),
                        ..Default::default()
                    }
                })
            }
            // TMDB find doesn't cover Season/Episode
            _ => None,
        };

        Ok(result_media.map(|m| MetaResult {
            media: m,
            relations: vec![],
        }))
    }
}
