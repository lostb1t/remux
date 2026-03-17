use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tracing::warn;

use crate::{AppContext, db, sdks};
use crate::sdks::CachedEndpoint;

use super::{MetaProvider, MetaResult};

pub struct TmdbMetaProvider;

const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";

fn tmdb_image(path: Option<&str>) -> Option<String> {
    path.filter(|p| !p.is_empty())
        .map(|p| format!("{}{}", TMDB_IMAGE_BASE, p))
}

#[async_trait]
impl MetaProvider for TmdbMetaProvider {
    async fn fetch(&self, media: &db::Media, ctx: &AppContext) -> Result<Option<MetaResult>> {
        // 1. Read API key from settings — skip if not configured
        let config = crate::db::Settings::get_config(&ctx.db).await?;
        let api_key = match config.tmdb_api_key.as_deref().filter(|k| !k.is_empty()) {
            Some(k) => k.to_string(),
            None => return Ok(None),
        };

        // 2. Resolve IMDB ID
        let imdb_id = match media.imdb_id.as_deref().or(media.series_imdb_id.as_deref()) {
            Some(id) => id.to_string(),
            None => return Ok(None),
        };

        // 3. TMDB Find-by-external-ID
        let client = sdks::RestClient::new("https://api.themoviedb.org/3/")?
            .with_auth(sdks::BearerAuth { token: api_key });

        let find_resp = client
            .execute(
                sdks::tmdb::FindByIdEndpoint {
                    external_id: imdb_id,
                    external_source: "imdb_id".to_string(),
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

        // 4. Map TMDB result → MetaResult based on media.kind
        let result_media = match media.kind {
            db::MediaKind::Movie => {
                find_resp.movie_results.into_iter().next().map(|m| db::Media {
                    title: m.title,
                    description: m.overview,
                    released_at: m
                        .release_date
                        .map(|d| d.and_hms_opt(0, 0, 0).unwrap()),
                    runtime: m.runtime.map(|r| r * 60), // minutes → seconds
                    rating_audience: m.vote_average,
                    poster: tmdb_image(m.poster_path.as_deref()),
                    backdrop: tmdb_image(m.backdrop_path.as_deref()),
                    ..Default::default()
                })
            }
            db::MediaKind::Series => {
                find_resp.tv_results.into_iter().next().map(|s| db::Media {
                    title: s.name,
                    description: s.overview,
                    released_at: s
                        .first_air_date
                        .map(|d| d.and_hms_opt(0, 0, 0).unwrap()),
                    rating_audience: s.vote_average,
                    poster: tmdb_image(s.poster_path.as_deref()),
                    backdrop: tmdb_image(s.backdrop_path.as_deref()),
                    ..Default::default()
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
