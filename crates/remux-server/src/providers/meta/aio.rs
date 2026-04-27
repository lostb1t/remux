use crate::{AppContext, db, sdks, utils};
use tracing::warn;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

use super::{MetaProvider, MetaRelation, MetaResult, TreeSyncProvider};

pub struct AioMetaProvider;

#[async_trait]
impl MetaProvider for AioMetaProvider {
    async fn fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<MetaResult>> {
        let imdb_id = media
            .series_media_id
            .clone()
            .or(media.external_ids.imdb.clone());

        let imdb_id = match imdb_id {
            Some(id) => id,
            None => return Ok(None),
        };

        let mut meta = if let Some(cached_meta) =
            ctx.store.get::<sdks::aio::Meta>(media.id.to_string())
        {
            cached_meta
        } else {
            crate::aio::AioService::from_settings(&ctx.db)
                .await?
                .get_meta(db::media_kind_to_aio(&media.kind), imdb_id.clone())
                .await?
        };

        if meta.imdb_id.is_none() {
            meta.imdb_id = db::ExternalIds::from_aio_id(&meta.id)
                .imdb
                .or_else(|| Some(imdb_id));
        }

        let meta_raw = meta.clone();
        let medias: Vec<db::Media> = db::aio_meta_to_medias(meta)?;
        let found = match media.kind {
            db::MediaKind::Movie => {
                medias.into_iter().find(|x| x.kind == db::MediaKind::Movie)
            }
            db::MediaKind::Series => {
                medias.into_iter().find(|x| x.kind == db::MediaKind::Series)
            }
            db::MediaKind::Season => {
                let idx = media.idx;
                medias
                    .into_iter()
                    .find(|x| x.kind == db::MediaKind::Season && x.idx == idx)
            }
            db::MediaKind::Episode => {
                let idx = media.idx;
                medias
                    .into_iter()
                    .find(|x| x.kind == db::MediaKind::Episode && x.idx == idx)
            }
            _ => None,
        };

        if let Some(mut found_media) = found {
            // Build relations for movies/series/episodes
            let relations = if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Series) {
                build_relations(media, &meta_raw)
            } else if media.kind == db::MediaKind::Episode {
                if let Some(meta_ep) = meta_raw.videos.as_ref().and_then(|v| {
                    v.iter().find(|e| {
                        e.episode == media.idx && e.season == media.parent_idx
                    })
                }) {
                    let rels = build_episode_relations(media, meta_ep);
                    warn!(id = %media.id, count = rels.len(), "aio episode relations");
                    rels
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

            let mut medias = vec![found_media];
            db::Media::enrich_parents(&ctx.db, &mut medias).await;
            found_media = medias.remove(0);

            Ok(Some(MetaResult {
                media: found_media,
                relations,
                season_posters: std::collections::HashMap::new(),
            }))
        } else {
            Ok(None)
        }
    }
}

pub struct AioTreeSyncProvider;

#[async_trait]
impl TreeSyncProvider for AioTreeSyncProvider {
    async fn get_seasons(
        &self,
        series: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let imdb_id = match series.external_ids.imdb.clone() {
            Some(id) => id,
            None => return Ok(vec![]),
        };

        let mut meta = crate::aio::AioService::from_settings(&ctx.db)
            .await?
            .get_meta(db::media_kind_to_aio(&series.kind), imdb_id.clone())
            .await?;

        if meta.imdb_id.is_none() {
            meta.imdb_id = db::ExternalIds::from_aio_id(&meta.id)
                .imdb
                .or_else(|| Some(imdb_id));
        }

        let meta_clone = meta.clone();
        let medias: Vec<db::Media> = db::aio_meta_to_medias(meta)?;
        let mut seasons = medias
            .into_iter()
            .filter_map(|mut x| {
                if x.kind == db::MediaKind::Season {
                    x.parent_id = Some(series.id);
                    x.series_id = Some(series.id);
                    x.poster = x.idx.and_then(|idx| meta_clone.get_season_poster(idx));
                    x.title = format!("Season {}", x.idx.unwrap_or(1));
                    x.refreshed_at = Some(Utc::now().naive_utc());
                    Some(x)
                } else {
                    None
                }
            })
            .collect();

        db::Media::enrich_parents(&ctx.db, &mut seasons).await;

        Ok(seasons)
    }

    async fn get_episodes(
        &self,
        season: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let imdb_id = match season.series_media_id.clone() {
            Some(id) => id,
            None => return Ok(vec![]),
        };

        let mut meta = crate::aio::AioService::from_settings(&ctx.db)
            .await?
            .get_meta(db::media_kind_to_aio(&season.kind), imdb_id.clone())
            .await?;

        if meta.imdb_id.is_none() {
            meta.imdb_id = db::ExternalIds::from_aio_id(&meta.id)
                .imdb
                .or_else(|| Some(imdb_id));
        }

        let medias: Vec<db::Media> = db::aio_meta_to_medias(meta)?;
        let mut episodes = medias
            .into_iter()
            .filter_map(|mut x| {
                if x.kind == db::MediaKind::Episode && x.parent_idx == season.idx {
                    x.parent_id = Some(season.id);
                    x.series_id = season.series_id;
                    if let Some(episode_num) = x.idx {
                        if let Some(season_num) = x.parent_idx {
                            x.title = format!(
                                "S{}E{} - {}",
                                season_num, episode_num, x.title
                            );
                        } else {
                            x.title = format!("E{} - {}", episode_num, x.title);
                        }
                    }
                    x.refreshed_at = Some(Utc::now().naive_utc());
                    Some(x)
                } else {
                    None
                }
            })
            .collect();

        db::Media::enrich_parents(&ctx.db, &mut episodes).await;

        Ok(episodes)
    }
}

/// Build Person/Genre MetaRelation entries from AIO metadata.
pub(crate) fn build_relations(media: &db::Media, meta: &sdks::aio::Meta) -> Vec<MetaRelation> {
    let mut relations: Vec<MetaRelation> = Vec::new();

    // Genres
    if let Some(genres) = meta.genre.as_ref().or(meta.genres.as_ref()) {
        for genre_name in genres {
            let genre_id =
                utils::get_stable_uuid(format!("genre:{}", genre_name.to_lowercase()));
            relations.push(MetaRelation {
                media: db::Media {
                    id: genre_id,
                    title: genre_name.clone(),
                    kind: db::MediaKind::Genre,
                    media_id: Some(format!("genre:{}", genre_name.to_lowercase())),
                    ..Default::default()
                },
                relation: db::MediaRelation {
                    left_media_id: media.id,
                    right_media_id: genre_id,
                    role: None,
                    ..Default::default()
                },
            });
        }
    }

    let mut relations = build_person_relations(
        media.id,
        meta.director.as_ref(), // Option<Vec<String>>
        meta.writer.as_ref(),
        None,               // cast_members: Option<Vec<CastMember>>
        meta.cast.as_ref(), // cast_names
        None,               // director_members
        None,               // writer_members
    );

    if let Some(extras) = &meta.app_extras {
        relations.extend(build_person_relations(
            media.id,
            None,
            None,
            extras.cast.as_ref(),
            None,
            extras.directors.as_ref(),
            extras.writers.as_ref(),
        ));
    }

    relations
}

pub(crate) fn build_episode_relations(
    media: &db::Media,
    ep: &sdks::aio::Episode,
) -> Vec<MetaRelation> {
    build_person_relations(
        media.id,
        ep.directors.as_ref(),
        ep.writers.as_ref(),
        None, // Skip cast to avoid generic series-level cast poisoning
        None,
        None,
        None,
    )
}

fn build_person_relations(
    left_media_id: Uuid,
    directors: Option<&Vec<String>>,
    writers: Option<&Vec<String>>,
    cast_members: Option<&Vec<sdks::aio::CastMember>>,
    cast_names: Option<&Vec<String>>,
    director_members: Option<&Vec<sdks::aio::CastMember>>,
    writer_members: Option<&Vec<sdks::aio::CastMember>>,
) -> Vec<MetaRelation> {
    let mut relations = Vec::new();

    let split_names = |names: Option<&Vec<String>>| -> Vec<String> {
        names
            .map(|v| v.as_slice())
            .unwrap_or_default()
            .iter()
            .flat_map(|s| s.split(',').map(|n| n.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect()
    };

    let mut add_members = |members: Option<&Vec<sdks::aio::CastMember>>,
                           role: db::RelationRole,
                           offset: i64| {
        if let Some(list) = members {
            for (i, member) in list.iter().enumerate() {
                if let Some(name) = &member.name {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        continue;
                    }
                    let person_id = utils::get_stable_uuid(format!(
                        "person:{}",
                        name.to_lowercase()
                    ));
                    relations.push(MetaRelation {
                        media: db::Media {
                            id: person_id,
                            title: name.clone(),
                            kind: db::MediaKind::Person,
                            poster: member.photo.clone(),
                            media_id: Some(format!("person:{}", name.to_lowercase())),
                            ..Default::default()
                        },
                        relation: db::MediaRelation {
                            left_media_id,
                            right_media_id: person_id,
                            weight: Some(offset + i as i64),
                            role: Some(role.clone()),
                            character: member.character.clone(),
                            ..Default::default()
                        },
                    });
                }
            }
        }
    };

    add_members(cast_members, db::RelationRole::Actor, 0);
    add_members(director_members, db::RelationRole::Director, 0);
    add_members(writer_members, db::RelationRole::Writer, 0);

    // Cast (actors from top-level)
    for (i, name) in split_names(cast_names).into_iter().enumerate() {
        let person_id =
            utils::get_stable_uuid(format!("person:{}", name.to_lowercase()));
        relations.push(MetaRelation {
            media: db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                media_id: Some(format!("person:{}", name.to_lowercase())),
                ..Default::default()
            },
            relation: db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some((i + cast_members.map(|c| c.len()).unwrap_or(0)) as i64),
                role: Some(db::RelationRole::Actor),
                ..Default::default()
            },
        });
    }

    // Directors
    for (i, name) in split_names(directors).into_iter().enumerate() {
        let person_id =
            utils::get_stable_uuid(format!("person:{}", name.to_lowercase()));
        relations.push(MetaRelation {
            media: db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                media_id: Some(format!("person:{}", name.to_lowercase())),
                ..Default::default()
            },
            relation: db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some(
                    (i + director_members.map(|c| c.len()).unwrap_or(0)) as i64,
                ),
                role: Some(db::RelationRole::Director),
                ..Default::default()
            },
        });
    }

    // Writers
    for (i, name) in split_names(writers).into_iter().enumerate() {
        let person_id =
            utils::get_stable_uuid(format!("person:{}", name.to_lowercase()));
        relations.push(MetaRelation {
            media: db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                media_id: Some(format!("person:{}", name.to_lowercase())),
                ..Default::default()
            },
            relation: db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some((i + writer_members.map(|c| c.len()).unwrap_or(0)) as i64),
                role: Some(db::RelationRole::Writer),
                ..Default::default()
            },
        });
    }

    relations
}
