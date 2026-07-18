use crate::OptionExt;
use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use axum_extra::extract::Query;
use remux_macros::{get, query};
use sqlx::QueryBuilder;
use uuid::Uuid;

use crate::{AppState, api, db, db::auth::AuthSession};

#[query]
#[derive(Debug)]
pub struct InstantMixQuery {
    pub user_id: Option<Uuid>,
    pub limit: Option<u32>,
}

#[query]
#[derive(Debug)]
pub struct InstantMixByIdQuery {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub limit: Option<u32>,
}

// ---------------------------------------------------------------------------
// Core helper
// ---------------------------------------------------------------------------

async fn genre_ids_for(db: &sqlx::SqlitePool, media_id: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT mr.right_media_id FROM media_relations mr \
         JOIN media g ON g.id = mr.right_media_id \
         WHERE mr.left_media_id = ? AND g.kind = 'genre'",
    )
    .bind(media_id)
    .fetch_all(db)
    .await
    .unwrap_or_default()
}

async fn related_artist_ids(db: &sqlx::SqlitePool, artist_ids: &[Uuid]) -> Vec<Uuid> {
    if artist_ids.is_empty() {
        return vec![];
    }

    let mut query = QueryBuilder::new(
        "SELECT related_media_id FROM artist_related WHERE artist_media_id IN (",
    );
    let mut separated = query.separated(", ");
    for id in artist_ids {
        separated.push_bind(id);
    }
    query.push(")");

    query
        .build_query_scalar::<Uuid>()
        .fetch_all(db)
        .await
        .unwrap_or_default()
}

async fn average_feature_vector(
    db: &sqlx::SqlitePool,
    seed_ids: &[Uuid],
) -> Result<Option<[f64; 9]>> {
    if seed_ids.is_empty() {
        return Ok(None);
    }

    let mut query = QueryBuilder::new(
        "SELECT \
            AVG(danceability), AVG(energy), AVG(valence), AVG(tempo), \
            AVG(acousticness), AVG(instrumentalness), AVG(loudness), \
            AVG(speechiness), AVG(liveness) \
         FROM media_features WHERE media_id IN (",
    );
    let mut separated = query.separated(", ");
    for id in seed_ids {
        separated.push_bind(id);
    }
    query.push(")");

    let row: (
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    ) = query
        .build_query_as()
        .fetch_one(db)
        .await?;

    Ok(match row {
        (
            Some(danceability),
            Some(energy),
            Some(valence),
            Some(tempo),
            Some(acousticness),
            Some(instrumentalness),
            Some(loudness),
            Some(speechiness),
            Some(liveness),
        ) => Some([
            danceability,
            energy,
            valence,
            tempo,
            acousticness,
            instrumentalness,
            loudness,
            speechiness,
            liveness,
        ]),
        _ => None,
    })
}

async fn build_feature_mix(
    ctx: &crate::AppContext,
    seed_ids: &[Uuid],
    genre_ids: &[Uuid],
    artist_ids: &[Uuid],
    include_related_artists: bool,
    limit: Option<u32>,
) -> Result<Option<Vec<db::Media>>> {
    let Some(vector) = average_feature_vector(&ctx.db, seed_ids).await? else {
        return Ok(None);
    };

    let mut candidate_artist_ids = artist_ids.to_vec();
    if include_related_artists {
        let related_ids = related_artist_ids(&ctx.db, artist_ids).await;
        if !related_ids.is_empty() {
            candidate_artist_ids = related_ids;
        }
        candidate_artist_ids.sort_unstable();
        candidate_artist_ids.dedup();
    }

    let mut query = QueryBuilder::new(
        "SELECT m.* FROM media m \
         JOIN media_features f ON f.media_id = m.id \
         WHERE m.kind = 'track'",
    );

    if !seed_ids.is_empty() {
        query.push(" AND m.id NOT IN (");
        let mut separated = query.separated(", ");
        for id in seed_ids {
            separated.push_bind(id);
        }
        query.push(")");
    }

    if !genre_ids.is_empty() {
        query.push(
            " AND EXISTS (\
                SELECT 1 FROM media_relations mr \
                WHERE mr.left_media_id = m.id \
                AND mr.right_media_id IN (",
        );
        let mut separated = query.separated(", ");
        for id in genre_ids {
            separated.push_bind(id);
        }
        query.push("))");
    }

    if !candidate_artist_ids.is_empty() {
        query.push(" AND m.grandparent_id IN (");
        let mut separated = query.separated(", ");
        for id in &candidate_artist_ids {
            separated.push_bind(id);
        }
        query.push(")");
    }

    let feature_columns = [
        "danceability",
        "energy",
        "valence",
        "tempo",
        "acousticness",
        "instrumentalness",
        "loudness",
        "speechiness",
        "liveness",
    ];

    query.push(" ORDER BY ");
    for (idx, (column, value)) in feature_columns
        .iter()
        .zip(vector)
        .enumerate()
    {
        if idx > 0 {
            query.push(" + ");
        }
        query
            .push("(f.")
            .push(*column)
            .push(" - ")
            .push_bind(value)
            .push(") * (f.")
            .push(*column)
            .push(" - ")
            .push_bind(value)
            .push(")");
    }
    query
        .push(", RANDOM() LIMIT ")
        .push_bind(limit.unwrap_or(50));

    let records: Vec<db::Media> = query
        .build_query_as()
        .fetch_all(&ctx.db)
        .await?;

    if records.is_empty() {
        Ok(None)
    } else {
        Ok(Some(records))
    }
}

async fn build_mix(
    ctx: &crate::AppContext,
    session: &AuthSession,
    seed_ids: Vec<Uuid>,
    genre_ids: Vec<Uuid>,
    artist_ids: Vec<Uuid>,
    limit: Option<u32>,
) -> Result<Vec<db::Media>> {
    use remux_sdks::remux::ItemSortBy;

    let (use_genre_ids, use_artist_ids) = if !genre_ids.is_empty() {
        (genre_ids, vec![])
    } else {
        (vec![], artist_ids)
    };

    let config = db::Settings::get_config_or_default(&ctx.db).await;
    if config
        .mix_audio_features
        .unwrap_or(false)
    {
        if let Some(records) = build_feature_mix(
            ctx,
            &seed_ids,
            &use_genre_ids,
            &use_artist_ids,
            config
                .mix_related_artists
                .unwrap_or(false),
            limit,
        )
        .await?
        {
            return Ok(records);
        }
    }

    let filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::Track]),
        genre_ids: if use_genre_ids.is_empty() {
            None
        } else {
            Some(use_genre_ids)
        },
        artist_ids: if use_artist_ids.is_empty() {
            None
        } else {
            Some(use_artist_ids)
        },
        sort_by: vec![ItemSortBy::Random],
        limit: Some(limit.unwrap_or(50)),
        include_user_state: true,
        user_id: Some(
            session
                .user
                .id,
        ),
        total_count: false,
        ..Default::default()
    };

    let result = db::Media::get_by_filter(&ctx.db, &filter).await?;
    Ok(result.records)
}

fn mix_response(items: Vec<db::Media>) -> impl IntoResponse {
    let total = items.len() as i64;
    let dtos: Vec<api::BaseItemDto> = items
        .into_iter()
        .map(|m| api::db_media_to_item(m, false))
        .collect();
    Json(api::BaseItemDtoQueryResult {
        items: dtos,
        total_record_count: total,
        start_index: 0,
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// GET /Songs/{itemId}/InstantMix
// ---------------------------------------------------------------------------

#[get("/songs/{item_id}/instantmix")]
pub async fn instant_mix_song(
    State(state): State<AppState>,
    session: AuthSession,
    Path(item_id): Path<Uuid>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    let track = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &item_id,
    )
    .await?
    .context_not_found("Song not found")?;

    let genre_ids = genre_ids_for(
        &state
            .ctx
            .db,
        track.id,
    )
    .await;
    let artist_ids = track
        .grandparent_id
        .into_iter()
        .collect();

    let items = build_mix(
        &state.ctx,
        &session,
        vec![track.id],
        genre_ids,
        artist_ids,
        q.limit,
    )
    .await?;
    Ok(mix_response(items))
}

// ---------------------------------------------------------------------------
// GET /Albums/{itemId}/InstantMix
// ---------------------------------------------------------------------------

#[get("/albums/{item_id}/instantmix")]
pub async fn instant_mix_album(
    State(state): State<AppState>,
    session: AuthSession,
    Path(item_id): Path<Uuid>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    let album = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &item_id,
    )
    .await?
    .context_not_found("Album not found")?;

    let genre_ids = genre_ids_for(
        &state
            .ctx
            .db,
        album.id,
    )
    .await;
    let artist_ids = album
        .parent_id
        .into_iter()
        .collect();

    let items = build_mix(
        &state.ctx,
        &session,
        vec![album.id],
        genre_ids,
        artist_ids,
        q.limit,
    )
    .await?;
    Ok(mix_response(items))
}

// ---------------------------------------------------------------------------
// GET /Artists/{itemId}/InstantMix
// ---------------------------------------------------------------------------

#[get("/artists/{item_id}/instantmix")]
pub async fn instant_mix_artist(
    State(state): State<AppState>,
    session: AuthSession,
    Path(item_id): Path<Uuid>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(
        &state
            .ctx
            .db,
        &item_id,
    )
    .await?
    .context_not_found("Artist not found")?;

    let items = build_mix(
        &state.ctx,
        &session,
        vec![item_id],
        vec![],
        vec![item_id],
        q.limit,
    )
    .await?;
    Ok(mix_response(items))
}

/// Legacy Jellyfin alias retained for clients that pass the artist as `Id`.
#[get("/artists/instantmix")]
pub async fn instant_mix_artist_by_id(
    State(state): State<AppState>,
    session: AuthSession,
    Query(q): Query<InstantMixByIdQuery>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(
        &state
            .ctx
            .db,
        &q.id,
    )
    .await?
    .filter(|item| item.kind == db::MediaKind::Artist)
    .context_not_found("Artist not found")?;
    let items = build_mix(
        &state.ctx,
        &session,
        vec![q.id],
        vec![],
        vec![q.id],
        q.limit,
    )
    .await?;
    Ok(mix_response(items))
}

// ---------------------------------------------------------------------------
// GET /Playlists/{itemId}/InstantMix
// ---------------------------------------------------------------------------

#[get("/playlists/{item_id}/instantmix")]
pub async fn instant_mix_playlist(
    State(state): State<AppState>,
    session: AuthSession,
    Path(item_id): Path<Uuid>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(
        &state
            .ctx
            .db,
        &item_id,
    )
    .await?
    .context_not_found("Playlist not found")?;

    // Fetch tracks in the playlist to gather their genres and artists.
    let tracks = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Track]),
            parent_id: Some(item_id),
            limit: Some(200),
            ..Default::default()
        },
    )
    .await?
    .records;

    let track_ids: Vec<Uuid> = tracks
        .iter()
        .map(|t| t.id)
        .collect();
    let artist_ids: Vec<Uuid> = tracks
        .iter()
        .filter_map(|t| t.grandparent_id)
        .collect();

    let genre_ids: Vec<Uuid> = if track_ids.is_empty() {
        vec![]
    } else {
        let mut ids = Vec::new();
        for tid in &track_ids {
            ids.extend(
                genre_ids_for(
                    &state
                        .ctx
                        .db,
                    *tid,
                )
                .await,
            );
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    };

    let items = build_mix(
        &state.ctx, &session, track_ids, genre_ids, artist_ids, q.limit,
    )
    .await?;
    Ok(mix_response(items))
}

// ---------------------------------------------------------------------------
// GET /Items/{itemId}/InstantMix  (dispatch by kind)
// ---------------------------------------------------------------------------

#[get("/items/{item_id}/instantmix")]
pub async fn instant_mix_item(
    State(state): State<AppState>,
    session: AuthSession,
    Path(item_id): Path<Uuid>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &item_id,
    )
    .await?
    .context_not_found("Item not found")?;

    let (genre_ids, artist_ids) = match media.kind {
        db::MediaKind::Track => {
            let g = genre_ids_for(
                &state
                    .ctx
                    .db,
                media.id,
            )
            .await;
            let a = media
                .grandparent_id
                .into_iter()
                .collect();
            (g, a)
        }
        db::MediaKind::Album => {
            let g = genre_ids_for(
                &state
                    .ctx
                    .db,
                media.id,
            )
            .await;
            let a = media
                .parent_id
                .into_iter()
                .collect();
            (g, a)
        }
        db::MediaKind::Artist => (vec![], vec![media.id]),
        db::MediaKind::Genre => (vec![media.id], vec![]),
        _ => {
            let g = genre_ids_for(
                &state
                    .ctx
                    .db,
                media.id,
            )
            .await;
            (g, vec![])
        }
    };

    let items = build_mix(
        &state.ctx,
        &session,
        vec![media.id],
        genre_ids,
        artist_ids,
        q.limit,
    )
    .await?;
    Ok(mix_response(items))
}

// ---------------------------------------------------------------------------
// GET /MusicGenres/{name}/InstantMix
// ---------------------------------------------------------------------------

#[get("/musicgenres/{name}/instantmix")]
pub async fn instant_mix_genre(
    State(state): State<AppState>,
    session: AuthSession,
    Path(name): Path<String>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    let genre = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM media WHERE kind IN ('genre', 'music_genre') AND LOWER(title) = LOWER(?) LIMIT 1",
    )
    .bind(&name)
    .fetch_optional(
        &state
            .ctx
            .db,
    )
    .await?
    .context_not_found("Genre not found")?;

    let items =
        build_mix(&state.ctx, &session, vec![], vec![genre], vec![], q.limit).await?;
    Ok(mix_response(items))
}

/// Jellyfin alias for clients that identify the music genre with an `Id` query.
#[get("/musicgenres/instantmix")]
pub async fn instant_mix_genre_by_id(
    State(state): State<AppState>,
    session: AuthSession,
    Query(q): Query<InstantMixByIdQuery>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(
        &state
            .ctx
            .db,
        &q.id,
    )
    .await?
    .filter(|item| {
        matches!(item.kind, db::MediaKind::Genre | db::MediaKind::MusicGenre)
    })
    .context_not_found("Genre not found")?;
    let items =
        build_mix(&state.ctx, &session, vec![], vec![q.id], vec![], q.limit).await?;
    Ok(mix_response(items))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use http::header::HeaderValue;

    use super::*;
    use crate::integration_test::{auth_header_with_token, authenticated_server};

    #[tokio::test]
    async fn legacy_id_routes_dispatch_to_existing_mix_engine() {
        let (server, guard, token) = authenticated_server().await;
        let now = Utc::now().naive_utc();
        let mut artist = db::Media {
            title: "Contract Artist".to_string(),
            kind: db::MediaKind::Artist,
            external_ids: db::ExternalIds {
                custom_stremio_id: Some("artist:contract-artist".to_string()),
                ..Default::default()
            },
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        artist
            .save(
                &guard
                    .0
                    .db,
            )
            .await
            .unwrap();
        let mut genre = db::Media {
            title: "Contract Genre".to_string(),
            kind: db::MediaKind::MusicGenre,
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        genre
            .save(
                &guard
                    .0
                    .db,
            )
            .await
            .unwrap();
        let auth = HeaderValue::from_str(&auth_header_with_token(&token)).unwrap();

        for endpoint in [
            format!("/artists/instantmix?Id={}", artist.id),
            format!("/musicgenres/instantmix?Id={}", genre.id),
        ] {
            let response = server
                .get(&endpoint)
                .add_header(http::header::AUTHORIZATION, auth.clone())
                .await;
            response.assert_status_ok();
            let body: serde_json::Value = response.json();
            assert!(body["Items"].is_array());
            assert!(body["StartIndex"].is_number());
            assert!(body["TotalRecordCount"].is_number());
        }
    }
}
