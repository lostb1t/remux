use std::time::Duration;

use crate::{AppContext, common, db, sdks};
use anyhow::Result;
use async_trait::async_trait;

use super::SearchService;

const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";

pub struct TmdbPersonSearchService;

#[async_trait]
impl SearchService for TmdbPersonSearchService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Person]
    }

    async fn search(
        &self,
        _kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let config = crate::db::Settings::get_config(&ctx.db).await?;
        let api_key = config.get_tmdb_key().to_string();

        let client = sdks::RestClient::new("https://api.themoviedb.org/3/")?
            .with_auth(sdks::BearerAuth { token: api_key });

        let resp = client
            .execute(sdks::tmdb::PersonSearchEndpoint {
                query: query.to_string(),
            })
            .await?;

        let media = resp
            .results
            .into_iter()
            .take(limit)
            .map(|p| {
                let media_id = format!("person:{}", p.name.to_lowercase());
                let id = common::get_stable_uuid(media_id.clone());
                let poster = p
                    .profile_path
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("{}{}", TMDB_IMAGE_BASE, s));
                let media = db::Media {
                    id,
                    title: p.name,
                    kind: db::MediaKind::Person,
                    poster,
                    media_id: Some(media_id),
                    ..Default::default()
                };
                ctx.store.insert(
                    id.to_string(),
                    media.clone(),
                    Duration::from_secs(3600),
                );
                media
            })
            .collect();

        Ok(media)
    }

    async fn persist(
        &self,
        id: uuid::Uuid,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        let mut media = match ctx.store.get::<db::Media>(id.to_string()) {
            Some(m) => m,
            None => return Ok(None),
        };

        media.save(&ctx.db).await.ok();
        ctx.store.delete(id.to_string());

        Ok(Some(media))
    }
}
