use anyhow::Result;
use futures::StreamExt;
use itertools::Itertools;
use std::time::Duration;
use tracing::{error, warn};
use uuid::Uuid;

use super::ProgressReporter;
use crate::sdks::CachedEndpoint;
use crate::{aio, db, sdks};

fn needs_release_dates(meta: &sdks::aio::Meta) -> bool {
    matches!(meta.media_type, sdks::aio::MediaType::Movie)
        && meta
            .app_extras
            .as_ref()
            .and_then(|e| e.release_dates.as_ref())
            .is_none()
}

fn inject_tmdb_release_dates(
    meta: &mut sdks::aio::Meta,
    tmdb_rd: sdks::tmdb::MovieReleaseDates,
) {
    let aio_rd = sdks::aio::ReleaseDates {
        results: tmdb_rd
            .results
            .into_iter()
            .map(|c| sdks::aio::ReleaseDateCountry {
                iso_3166_1: c.iso_3166_1,
                release_dates: c
                    .release_dates
                    .into_iter()
                    .filter_map(|rd| {
                        rd.release_date.map(|date| sdks::aio::ReleaseDateEntry {
                            release_date: date,
                            release_type: rd.release_type,
                        })
                    })
                    .collect(),
            })
            .collect(),
    };
    meta.app_extras
        .get_or_insert_with(Default::default)
        .release_dates = Some(aio_rd);
}

/// Ensure `meta.imdb_id` is set and, for movies, `app_extras.release_dates` is
/// populated so `digital_released_at` can be computed on DB conversion.
///
/// imdb_id resolution order:
/// 1. Already set.
/// 2. Parse `meta.id` prefix for an IMDb ID (`tt…`).
/// 3. `meta.resolve()` via AIO — upgrades partial stubs to full metadata.
/// 4. TMDB detail lookup using a TMDB id from the meta.
/// 5. TVDB fallback via TMDB find-by-external-id.
///
/// After imdb_id is confirmed, movies always get a TMDB release-date lookup
/// when `app_extras.release_dates` is still missing (TMDB responses are cached).
///
/// Returns `true` if `meta.imdb_id` is set after all attempts.
pub async fn resolve_imdb_id<A: sdks::Auth + Clone>(
    meta: &mut sdks::aio::Meta,
    aio: Option<&aio::AioService>,
    tmdb_client: Option<&sdks::RestClient<A>>,
) -> bool {
    // --- Phase 1: resolve imdb_id ---

    if meta.imdb_id.is_none() {
        if let Some(imdb) = db::ExternalIds::from_aio_id(&meta.id).imdb {
            meta.imdb_id = Some(imdb);
        }
    }

    if meta.imdb_id.is_none() {
        if let Some(aio) = aio {
            match meta.resolve(&aio.client).await {
                Ok(()) => {}
                Err(e) => warn!(id = %meta.id, error = %e, "AIO resolve failed"),
            }
        }
    }

    if meta.imdb_id.is_none() {
        let external_ids = db::ExternalIds::from_aio_id(&meta.id);
        let tmdb_id = external_ids
            .tmdb
            .or_else(|| meta.moviedb_id.map(|n| n as i64));

        if let (Some(client), Some(tid)) = (tmdb_client, tmdb_id) {
            match meta.media_type {
                sdks::aio::MediaType::Movie => {
                    if let Ok(movie) = client
                        .execute(
                            sdks::tmdb::MovieEndpoint::new(tid)
                                .with_cache(Duration::from_secs(3600)),
                        )
                        .await
                    {
                        if needs_release_dates(meta) {
                            if let Some(rd) = movie.release_dates {
                                inject_tmdb_release_dates(meta, rd);
                            }
                        }
                        meta.imdb_id = movie.imdb_id;
                    }
                }
                sdks::aio::MediaType::Series => {
                    meta.imdb_id = client
                        .execute(
                            sdks::tmdb::SeriesEndpoint::new(tid)
                                .with_cache(Duration::from_secs(3600)),
                        )
                        .await
                        .ok()
                        .and_then(|s| s.external_ids)
                        .and_then(|e| e.imdb_id);
                }
                _ => {}
            }
        }

        if meta.imdb_id.is_none() {
            if let (Some(client), Some(tvdb)) = (tmdb_client, external_ids.tvdb) {
                let find_resp = client
                    .execute(
                        sdks::tmdb::FindByIdEndpoint {
                            external_id: tvdb.to_string(),
                            external_source: "tvdb_id".to_string(),
                        }
                        .with_cache(Duration::from_secs(3600)),
                    )
                    .await
                    .ok();

                meta.imdb_id = find_resp.and_then(|r| match meta.media_type {
                    sdks::aio::MediaType::Movie => {
                        r.movie_results.into_iter().next().and_then(|m| m.imdb_id)
                    }
                    sdks::aio::MediaType::Series => r
                        .tv_results
                        .into_iter()
                        .next()
                        .and_then(|s| s.external_ids)
                        .and_then(|e| e.imdb_id),
                    _ => None,
                });
            }
        }
    }

    if meta.imdb_id.is_none() {
        return false;
    }

    // --- Phase 2: fill movie release dates from TMDB if still missing ---

    if needs_release_dates(meta) {
        if let Some(client) = tmdb_client {
            // Prefer a direct TMDB id; fall back to IMDb→TMDB lookup.
            let tmdb_id = db::ExternalIds::from_aio_id(&meta.id)
                .tmdb
                .or_else(|| meta.moviedb_id.map(|n| n as i64));

            let tmdb_id = if tmdb_id.is_some() {
                tmdb_id
            } else if let Some(ref imdb_id) = meta.imdb_id {
                client
                    .execute(
                        sdks::tmdb::FindByIdEndpoint {
                            external_id: imdb_id.clone(),
                            external_source: "imdb_id".to_string(),
                        }
                        .with_cache(Duration::from_secs(3600)),
                    )
                    .await
                    .ok()
                    .and_then(|r| r.movie_results.into_iter().next())
                    .map(|m| m.id)
            } else {
                None
            };

            if let Some(tid) = tmdb_id {
                if let Ok(movie) = client
                    .execute(
                        sdks::tmdb::MovieEndpoint::new(tid)
                            .with_cache(Duration::from_secs(3600)),
                    )
                    .await
                {
                    if let Some(rd) = movie.release_dates {
                        inject_tmdb_release_dates(meta, rd);
                    }
                }
            }
        }
    }

    true
}

/// Consume `stream`, resolving IMDb IDs and hydrating full metadata via AIO,
/// then converting, upserting, and relating items to `catalog_id`.
/// Returns the count of top-level items imported.
pub async fn import_catalog_items<S, A>(
    db: &sqlx::SqlitePool,
    catalog_id: Uuid,
    media_id: &str,
    max: usize,
    stream: S,
    aio: Option<&aio::AioService>,
    tmdb_client: Option<&sdks::RestClient<A>>,
    progress: &ProgressReporter,
) -> Result<usize>
where
    S: futures::Stream<Item = sdks::aio::Meta> + Unpin,
    A: sdks::Auth + Clone,
{
    let mut meta_stream = stream.chunks(500);
    let mut count = 0usize;

    while let Some(mut metas) = meta_stream.next().await {
        progress.set(count as f64 / max.max(1) as f64 * 100.0);

        let remaining = max.saturating_sub(count);
        if remaining == 0 {
            break;
        }
        metas = metas.into_iter().take(remaining).collect();

        let items: Vec<db::Media> =
            futures::stream::iter(metas.into_iter().unique_by(|meta| meta.id.clone()))
                .then(|mut meta| async move {
                    if !resolve_imdb_id(&mut meta, aio, tmdb_client).await {
                        warn!(id = %meta.id, "could not resolve imdb_id, skipping");
                        return vec![];
                    }
                    match db::aio_meta_to_medias(meta) {
                        Ok(mut items) => {
                            if let Some(top) = items.first_mut() {
                                top.parent_id = None;
                            }
                            items
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to convert metadata, skipping");
                            vec![]
                        }
                    }
                })
                .flat_map(futures::stream::iter)
                .collect()
                .await;

        if items.is_empty() {
            break;
        }

        if let Err(e) = db::Media::upsert(db, &items).await {
            error!(catalog = media_id, error = %e, "failed to import chunk");
            continue;
        }

        let relations: Vec<db::MediaRelation> = items
            .iter()
            .filter(|m| m.parent_id.is_none())
            .map(|m| db::MediaRelation {
                left_media_id: m.id,
                right_media_id: catalog_id,
                role: Some(db::RelationRole::Catalog),
                ..Default::default()
            })
            .collect();

        if !relations.is_empty() {
            if let Err(e) = db::MediaRelation::upsert(db, &relations).await {
                error!(catalog = media_id, error = %e, "failed to upsert catalog relations");
            }
        }

        count += items.iter().filter(|m| m.parent_id.is_none()).count();
        if count >= max {
            break;
        }
    }

    Ok(count)
}
