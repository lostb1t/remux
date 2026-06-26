use std::collections::HashSet;

use axum::{Json, extract::State, response::IntoResponse};
use axum_anyhow::ApiResult as Result;
use axum_extra::extract::Query;
use remux_macros::{get, query};
use uuid::Uuid;

use crate::{AppState, api, db, db::auth::AuthSession};

#[query]
#[derive(Debug, Default)]
pub struct GetMovieRecommendationsQuery {
    pub user_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub category_limit: Option<u32>,
    pub item_limit: Option<u32>,
}

#[get("/movies/recommendations")]
pub async fn movies_recommendations(
    State(state): State<AppState>,
    session: AuthSession,
    Query(q): Query<GetMovieRecommendationsQuery>,
) -> Result<impl IntoResponse> {
    let user_id = q
        .user_id
        .unwrap_or(
            session
                .user
                .id,
        );
    let categories = build_recommendations(
        &state
            .ctx
            .db,
        user_id,
        q.parent_id,
        db::MediaKind::Movie,
        q.category_limit
            .unwrap_or(5) as usize,
        q.item_limit
            .unwrap_or(8),
    )
    .await?;
    Ok(Json(categories))
}

pub(super) async fn build_recommendations(
    db: &sqlx::SqlitePool,
    user_id: Uuid,
    parent_id: Option<Uuid>,
    kind: db::MediaKind,
    category_limit: usize,
    item_limit: u32,
) -> Result<Vec<api::RecommendationDto>> {
    // Recently played (up to 7), ordered by last played date.
    let recently_played = db::Media::get_by_filter(
        db,
        &db::MediaFilter {
            kind: Some(vec![kind.clone()]),
            parent_id,
            recursive: parent_id.is_some(),
            user_id: Some(user_id),
            user_state: Some(db::UserMediaStateFilter {
                user_id: Some(user_id),
                played: Some(true),
                ..Default::default()
            }),
            sort_by: vec![api::ItemSortBy::DatePlayed],
            sort_order: vec![api::SortOrder::Descending],
            limit: Some(7),
            total_count: false,
            ..Default::default()
        },
    )
    .await?
    .records;

    let recently_played_ids: HashSet<Uuid> = recently_played
        .iter()
        .map(|m| m.id)
        .collect();

    // Liked/favorited items (up to 10), random order, excluding recently played.
    let liked: Vec<_> = db::Media::get_by_filter(
        db,
        &db::MediaFilter {
            kind: Some(vec![kind.clone()]),
            parent_id,
            recursive: parent_id.is_some(),
            user_state: Some(db::UserMediaStateFilter {
                user_id: Some(user_id),
                favorite: Some(true),
                ..Default::default()
            }),
            sort_by: vec![api::ItemSortBy::Random],
            limit: Some(10),
            total_count: false,
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .filter(|m| !recently_played_ids.contains(&m.id))
    .collect();

    // Build SimilarToRecentlyPlayed and SimilarToLikedItem categories, genre-filtered per baseline.
    let similar_recent = build_similar_categories(
        db,
        &recently_played,
        kind.clone(),
        parent_id,
        item_limit,
        api::RecommendationType::SimilarToRecentlyPlayed,
    )
    .await?;
    let similar_liked = build_similar_categories(
        db,
        &liked,
        kind.clone(),
        parent_id,
        item_limit,
        api::RecommendationType::SimilarToLikedItem,
    )
    .await?;

    // Build HasActorFromRecentlyPlayed categories from top 6 recently played movies.
    let actor_cats =
        build_actor_categories(db, &recently_played, kind, parent_id, item_limit)
            .await?;

    // Round-robin interleave: 2 from recent, 2 from liked, 1 from actor per cycle.
    let mut result = Vec::with_capacity(category_limit);
    let mut ri = 0usize;
    let mut li = 0usize;
    let mut ai = 0usize;

    'outer: loop {
        let mut any = false;
        for _ in 0..2 {
            if ri < similar_recent.len() {
                result.push(similar_recent[ri].clone());
                ri += 1;
                any = true;
                if result.len() >= category_limit {
                    break 'outer;
                }
            }
        }
        for _ in 0..2 {
            if li < similar_liked.len() {
                result.push(similar_liked[li].clone());
                li += 1;
                any = true;
                if result.len() >= category_limit {
                    break 'outer;
                }
            }
        }
        if ai < actor_cats.len() {
            result.push(actor_cats[ai].clone());
            ai += 1;
            any = true;
            if result.len() >= category_limit {
                break 'outer;
            }
        }
        if !any {
            break;
        }
    }

    Ok(result)
}

async fn build_similar_categories(
    db: &sqlx::SqlitePool,
    baselines: &[db::Media],
    kind: db::MediaKind,
    parent_id: Option<Uuid>,
    item_limit: u32,
    rec_type: api::RecommendationType,
) -> Result<Vec<api::RecommendationDto>> {
    let mut cats = Vec::new();
    for baseline in baselines {
        let mut baseline = baseline.clone();
        baseline
            .load_relations(db)
            .await?;
        let genre_ids: Vec<Uuid> = baseline
            .relations
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter(|(_, m)| m.kind == db::MediaKind::Genre)
            .map(|(_, m)| m.id)
            .collect();
        if genre_ids.is_empty() {
            continue;
        }
        let items: Vec<_> = db::Media::get_by_filter(
            db,
            &db::MediaFilter {
                kind: Some(vec![kind.clone()]),
                parent_id,
                recursive: parent_id.is_some(),
                genre_ids: Some(genre_ids),
                sort_by: vec![api::ItemSortBy::CommunityRating],
                sort_order: vec![api::SortOrder::Descending],
                limit: Some(item_limit + 1),
                total_count: false,
                ..Default::default()
            },
        )
        .await?
        .records
        .into_iter()
        .filter(|m| m.id != baseline.id)
        .take(item_limit as usize)
        .map(|m| api::db_media_to_item(m, false))
        .collect();
        if items.is_empty() {
            continue;
        }
        cats.push(api::RecommendationDto {
            category_id: Some(baseline.id),
            recommendation_type: rec_type,
            baseline_item_name: Some(
                baseline
                    .title
                    .clone(),
            ),
            baseline_item_id: Some(baseline.id),
            items,
        });
    }
    Ok(cats)
}

async fn build_actor_categories(
    db: &sqlx::SqlitePool,
    recently_played: &[db::Media],
    kind: db::MediaKind,
    parent_id: Option<Uuid>,
    item_limit: u32,
) -> Result<Vec<api::RecommendationDto>> {
    let mut cats = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for movie in recently_played
        .iter()
        .take(6)
        .cloned()
    {
        let mut movie = movie;
        movie
            .load_relations(db)
            .await?;
        let Some(rels) = movie.relations else {
            continue;
        };

        for (rel, person) in rels {
            // Top 3 actors only (weight 0, 1, 2), deduplicated by name.
            if rel.role != Some(db::RelationRole::Actor) {
                continue;
            }
            if rel
                .weight
                .unwrap_or(999)
                > 2
            {
                continue;
            }
            if !seen.insert(
                person
                    .title
                    .clone(),
            ) {
                continue;
            }

            let items: Vec<_> = db::Media::get_by_filter(
                db,
                &db::MediaFilter {
                    kind: Some(vec![kind.clone()]),
                    parent_id,
                    recursive: parent_id.is_some(),
                    person_ids: Some(vec![person.id]),
                    sort_by: vec![api::ItemSortBy::CommunityRating],
                    sort_order: vec![api::SortOrder::Descending],
                    limit: Some(item_limit + 2),
                    total_count: false,
                    ..Default::default()
                },
            )
            .await?
            .records
            .into_iter()
            .take(item_limit as usize)
            .map(|m| api::db_media_to_item(m, false))
            .collect();

            if !items.is_empty() {
                cats.push(api::RecommendationDto {
                    category_id: Some(Uuid::new_v5(
                        &Uuid::NAMESPACE_OID,
                        person
                            .title
                            .as_bytes(),
                    )),
                    recommendation_type:
                        api::RecommendationType::HasActorFromRecentlyPlayed,
                    baseline_item_name: Some(
                        person
                            .title
                            .clone(),
                    ),
                    baseline_item_id: None,
                    items,
                });
            }
        }
    }

    Ok(cats)
}
