use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::{Json, extract::State, response::IntoResponse};
use axum_anyhow::ApiResult as Result;
use axum_extra::extract::Query;
use moka::sync::Cache;
use rand::seq::SliceRandom;
use rand::{SeedableRng, rngs::StdRng};
use remux_macros::{api_query, get, query};
use sqlx::QueryBuilder;
use uuid::Uuid;

use crate::{AppState, api, db, db::auth::AuthSession};

#[query]
#[derive(Debug, Default)]
pub struct GetMovieRecommendationsQuery {
    pub user_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub category_limit: Option<u32>,
    pub item_limit: Option<u32>,
    pub shuffle: Option<bool>,
    pub shuffle_seed: Option<u64>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RecommendationCacheKey {
    user_id: Uuid,
    parent_id: Option<Uuid>,
    kind: String,
    category_limit: usize,
    item_limit: u32,
    shuffle: bool,
    shuffle_seed: u64,
}

static RECOMMENDATION_CACHE: LazyLock<Cache<RecommendationCacheKey, Vec<api::RecommendationDto>>> =
    LazyLock::new(|| {
        Cache::builder()
            .max_capacity(256)
            .time_to_live(Duration::from_secs(60))
            .build()
    });

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
    let category_limit = q
        .category_limit
        .unwrap_or(5) as usize;
    let item_limit = q
        .item_limit
        .unwrap_or(8);
    let started = std::time::Instant::now();
    let categories = build_recommendations(
        &state
            .ctx
            .db,
        user_id,
        q.parent_id,
        db::MediaKind::Movie,
        category_limit,
        item_limit,
        q.shuffle
            .unwrap_or(false),
        q.shuffle_seed
            .unwrap_or(0),
    )
    .await?;
    tracing::debug!(
        target: "remux_server::recommendations",
        kind = "Movie",
        user_id = %user_id,
        category_limit = category_limit,
        item_limit = item_limit,
        category_count = categories.len(),
        item_count = categories.iter().map(|category| category.items.len()).sum::<usize>(),
        elapsed_ms = started.elapsed().as_millis(),
        "built recommendations",
    );
    Ok(Json(categories))
}

pub async fn build_recommendations(
    db: &sqlx::SqlitePool,
    user_id: Uuid,
    parent_id: Option<Uuid>,
    kind: db::MediaKind,
    category_limit: usize,
    item_limit: u32,
    shuffle: bool,
    shuffle_seed: u64,
) -> Result<Vec<api::RecommendationDto>> {
    let recommendations_started_at = Instant::now();
    let cache_key = RecommendationCacheKey {
        user_id,
        parent_id,
        kind: kind.to_string(),
        category_limit,
        item_limit,
        shuffle,
        shuffle_seed,
    };
    if let Some(cached) = RECOMMENDATION_CACHE.get(&cache_key) {
        tracing::debug!(
            target: "remux_server::recommendations",
            kind = ?kind,
            user_id = %user_id,
            category_limit,
            item_limit,
            elapsed_ms = recommendations_started_at.elapsed().as_millis(),
            result_categories = cached.len(),
            result_items = cached.iter().map(|category| category.items.len()).sum::<usize>(),
            "recommendation cache hit"
        );
        return Ok(cached);
    }
    let profile_started_at = Instant::now();
    // Recently played (up to 7), ordered by last played date.
    let recently_played =
        gather_recently_consumed(db, user_id, kind.clone(), parent_id, 7).await?;
    let mut unique_recently_played: Vec<db::Media> = Vec::new();
    let mut recent_seen_ids = HashSet::new();
    for media in recently_played {
        if recent_seen_ids.insert(media.id) {
            unique_recently_played.push(media);
        }
    }
    let recently_played = unique_recently_played;

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

    let mut baseline_tag_values: Vec<&String> = Vec::new();
    baseline_tag_values.extend(
        recently_played
            .iter()
            .chain(liked.iter())
            .flat_map(|media| {
                media
                    .tags
                    .iter()
            }),
    );
    let mut baseline_tags = recommendation_profile_tags(
        baseline_tag_values
            .iter()
            .copied(),
        MAX_RECOMMENDATION_PROFILE_TAGS,
        MIN_RECOMMENDATION_PROFILE_TAG_APPEARANCES,
    );
    let has_segmented_recommendation_profile = segmented_profile_seed_count(
        &recently_played,
        &liked,
    ) >= MIN_SEGMENTED_RECOMMENDATION_PROFILE_ITEMS;
    if !has_segmented_recommendation_profile {
        baseline_tags.retain(|tag| !SEGMENTED_RECOMMENDATION_TAGS.contains(&tag.as_str()));
    }

    let min_similarity_overlap = if baseline_tags.len() >= 8 { 3 } else { 2 };
    let profile_elapsed_ms = profile_started_at.elapsed().as_millis();

    let title_rows_started_at = Instant::now();
    // Build SimilarToRecentlyPlayed and SimilarToLikedItem categories, genre-filtered per baseline.
    let mut seen_item_ids: HashSet<Uuid> = recently_played
        .iter()
        .chain(liked.iter())
        .map(|m| m.id)
        .collect();
    let excluded_title_keys: HashSet<String> = recently_played
        .iter()
        .chain(liked.iter())
        .map(|m| recommendation_title_key(&m.title))
        .collect();
    let title_row_min_items = std::cmp::min(3, item_limit as usize);
    let similar_recent_raw = build_similar_categories(
        db,
        user_id,
        &excluded_title_keys,
        &recently_played,
        &baseline_tags,
        kind.clone(),
        parent_id,
        item_limit,
        min_similarity_overlap,
        false,
        1,
        api::RecommendationType::SimilarToRecentlyPlayed,
        &mut seen_item_ids,
    )
    .await?;
    let similar_liked_raw = build_similar_categories(
        db,
        user_id,
        &excluded_title_keys,
        &liked,
        &baseline_tags,
        kind.clone(),
        parent_id,
        item_limit,
        min_similarity_overlap,
        false,
        1,
        api::RecommendationType::SimilarToLikedItem,
        &mut seen_item_ids,
    )
    .await?;
    let (similar_recent, merged_recent_items, merged_recent_sources) =
        split_thin_title_categories(similar_recent_raw, title_row_min_items);
    let (similar_liked, merged_liked_items, merged_liked_sources) =
        split_thin_title_categories(similar_liked_raw, title_row_min_items);
    let title_rows_elapsed_ms = title_rows_started_at.elapsed().as_millis();

    let has_content_based_similarities =
        !similar_recent.is_empty() || !similar_liked.is_empty();
    let similar_row_count = similar_recent.len() + similar_liked.len();
    let focus_baseline_count = similar_recent
        .iter()
        .chain(similar_liked.iter())
        .filter_map(|category| category.baseline_item_id)
        .collect::<HashSet<_>>()
        .len();
    let allow_persona_rows = has_content_based_similarities
        && similar_row_count >= 3
        && focus_baseline_count >= 2
        && baseline_tags.len() >= 2;
    let actor_min_appearances = if recently_played.len() >= 8 {
        5
    } else if recently_played.len() >= 4 {
        4
    } else {
        3
    };
    let director_min_appearances = if recently_played.len() >= 6 {
        3
    } else if recently_played.len() >= 4 {
        2
    } else {
        1
    };
    let actor_profile_overlap = if baseline_tags.is_empty() {
        1
    } else if baseline_tags.len() >= 8 {
        3
    } else {
        2
    };
    let actor_min_distinct_genres = if baseline_tags.len() >= 6 { 3 } else { 2 };
    let actor_row_item_limit = min(item_limit, std::cmp::max(4, item_limit / 2));
    let director_row_item_limit = min(item_limit, 5);

    let taste_rows_started_at = Instant::now();
    let fallback_taste_categories = filter_categories_by_min_items(
        build_taste_similarity_categories(
            db,
            user_id,
            &excluded_title_keys,
            &recently_played,
            &liked,
            &baseline_tags,
            has_segmented_recommendation_profile,
            kind.clone(),
            parent_id,
            item_limit,
            std::cmp::max(3, category_limit),
            &mut seen_item_ids,
        )
        .await?,
        title_row_min_items,
    );
    let taste_rows_elapsed_ms = taste_rows_started_at.elapsed().as_millis();

    let person_rows_started_at = Instant::now();
    // Build role-based categories from top recently played media, then interleave with similar rows.
    let actor_cats = if allow_persona_rows {
        let actor_category_limit = 1;
        build_person_categories(
            db,
            user_id,
            &excluded_title_keys,
            &recently_played,
            &liked,
            &baseline_tags,
            kind.clone(),
            parent_id,
            actor_row_item_limit,
            db::RelationRole::Actor,
            api::RecommendationType::HasActorFromRecentlyPlayed,
            actor_min_appearances,
            actor_category_limit,
            2,
            std::cmp::max(actor_profile_overlap, 3),
            actor_profile_overlap,
            actor_min_distinct_genres,
            &mut seen_item_ids,
        )
        .await?
    } else {
        Vec::new()
    };
    let director_cats = if allow_persona_rows {
        let director_category_limit = 1;
        build_person_categories(
            db,
            user_id,
            &excluded_title_keys,
            &recently_played,
            &liked,
            &baseline_tags,
            kind.clone(),
            parent_id,
            director_row_item_limit,
            db::RelationRole::Director,
            api::RecommendationType::HasDirectorFromRecentlyPlayed,
            director_min_appearances,
            director_category_limit,
            2,
            min_profile_overlap_threshold(&baseline_tags),
            min_profile_overlap_threshold(&baseline_tags),
            2,
            &mut seen_item_ids,
        )
        .await?
    } else {
        Vec::new()
    };
    let person_rows_elapsed_ms = person_rows_started_at.elapsed().as_millis();

    let assembly_started_at = Instant::now();
    // Interleave high-confidence title/person rows with broader taste rows.
    // Title-based rows are useful UX anchors, but only after their own builder
    // proves they have enough cards. Broader taste rows fill quantity and keep
    // the page from becoming a list of thin "because you watched X" shelves.
    let merged_recent_category = merged_similarity_category(
        merged_recent_items,
        item_limit,
        api::RecommendationType::SimilarToRecentlyPlayed,
        &merged_recent_sources,
        "recently watched titles",
        b"remux-merged-recent-similar",
    );
    let merged_liked_category = merged_similarity_category(
        merged_liked_items,
        item_limit,
        api::RecommendationType::SimilarToLikedItem,
        &merged_liked_sources,
        "favorite titles",
        b"remux-merged-liked-similar",
    );
    let mut merged_cats = Vec::new();
    if let Some(category) = merged_recent_category {
        merged_cats.push(category);
    }
    if let Some(category) = merged_liked_category {
        merged_cats.push(category);
    }

    let mut result = Vec::with_capacity(category_limit);
    let mut ri = 0usize;
    let mut li = 0usize;
    let mut fi = 0usize;
    let mut mi = 0usize;
    let mut ai = 0usize;
    let mut di = 0usize;

    'outer: loop {
        let mut any = false;

        if ri < similar_recent.len() {
            result.push(similar_recent[ri].clone());
            ri += 1;
            any = true;
            if result.len() >= category_limit {
                break 'outer;
            }
        }

        if fi < fallback_taste_categories.len() {
            result.push(fallback_taste_categories[fi].clone());
            fi += 1;
            any = true;
            if result.len() >= category_limit {
                break 'outer;
            }
        }

        if li < similar_liked.len() {
            result.push(similar_liked[li].clone());
            li += 1;
            any = true;
            if result.len() >= category_limit {
                break 'outer;
            }
        }

        if mi < merged_cats.len() {
            result.push(merged_cats[mi].clone());
            mi += 1;
            any = true;
            if result.len() >= category_limit {
                break 'outer;
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

        if di < director_cats.len() {
            result.push(director_cats[di].clone());
            di += 1;
            any = true;
            if result.len() >= category_limit {
                break 'outer;
            }
        }

        if !any {
            break;
        }
    }

    while result.len() < category_limit && fi < fallback_taste_categories.len() {
        result.push(fallback_taste_categories[fi].clone());
        fi += 1;
    }

    while result.len() < category_limit && mi < merged_cats.len() {
        result.push(merged_cats[mi].clone());
        mi += 1;
    }

    while result.len() < category_limit && ri < similar_recent.len() {
        result.push(similar_recent[ri].clone());
        ri += 1;
    }

    while result.len() < category_limit && li < similar_liked.len() {
        result.push(similar_liked[li].clone());
        li += 1;
    }

    result = dedupe_recommendation_items(result);
    result = dedupe_recommendation_categories(result);

    if shuffle {
        let mut rng = StdRng::seed_from_u64(shuffle_seed);
        for category in &mut result {
            category
                .items
                .shuffle(&mut rng);
        }
        result.shuffle(&mut rng);
    }

    let assembly_elapsed_ms = assembly_started_at.elapsed().as_millis();
    tracing::debug!(
        target: "remux_server::recommendations",
        kind = ?kind,
        user_id = %user_id,
        category_limit,
        item_limit,
        profile_elapsed_ms,
        title_rows_elapsed_ms,
        taste_rows_elapsed_ms,
        person_rows_elapsed_ms,
        assembly_elapsed_ms,
        total_elapsed_ms = recommendations_started_at.elapsed().as_millis(),
        result_categories = result.len(),
        result_items = result.iter().map(|category| category.items.len()).sum::<usize>(),
        "recommendation build stages"
    );

    RECOMMENDATION_CACHE.insert(cache_key, result.clone());
    Ok(result)
}

fn dedupe_recommendation_items(
    mut categories: Vec<api::RecommendationDto>,
) -> Vec<api::RecommendationDto> {
    let mut seen_items = HashSet::new();
    let mut filtered: Vec<api::RecommendationDto> =
        Vec::with_capacity(categories.len());

    for mut category in categories.drain(..) {
        let items = std::mem::take(&mut category.items)
            .into_iter()
            .filter(|item| {
                let id = item
                    .id
                    .to_string();
                seen_items.insert(id.to_lowercase())
            })
            .collect();
        category.items = items;
        if !category
            .items
            .is_empty()
        {
            filtered.push(category);
        }
    }

    filtered
}

fn dedupe_recommendation_categories(
    mut categories: Vec<api::RecommendationDto>,
) -> Vec<api::RecommendationDto> {
    let mut seen: HashSet<(String, Option<Uuid>, String)> = HashSet::new();
    categories.retain(|category| {
        let rec_type = format!("{:?}", category.recommendation_type);
        let baseline_id = category
            .baseline_item_id
            .or(category.category_id);
        let baseline_name = category
            .baseline_item_name
            .as_deref()
            .unwrap_or("")
            .to_string();
        seen.insert((rec_type, baseline_id, baseline_name))
    });

    categories
}

const BANNED_RECOMMENDATION_TAGS: [&str; 11] = [
    "tmdb",
    "imdb",
    "tvdb",
    "provider",
    "language",
    "format",
    "subtitle",
    "audio",
    "country",
    "collection",
    "tag:",
];

const MAX_RECOMMENDATION_PROFILE_TAGS: usize = 20;
const MIN_RECOMMENDATION_PROFILE_TAG_APPEARANCES: usize = 2;
const MIN_SHARED_TAGS_FOR_STRONG_MATCH: usize = 2;
const MIN_GLOBAL_RECOMMENDATION_TAGS: usize = 2;
const MIN_SIMILAR_ITEMS_PER_BASELINE: usize = 2;
const TITLE_STANDALONE_FALLBACK_MIN_ITEMS: usize = 3;
const MIN_PERSON_GLOBAL_TAG_OVERLAP: usize = 2;
const MIN_PERSON_BASELINE_TAG_OVERLAP_ACTOR: usize = 3;
const SIGNAL_SCORE_TITLE_MATCH: f64 = 6.4;
const SIGNAL_SCORE_GENRE_MATCH: f64 = 5.2;
const SIGNAL_SCORE_TAG_MATCH: f64 = 5.0;
const SIGNAL_SCORE_PERSON_MATCH: f64 = 4.0;
const RECOMMENDATION_SCORE_GENRE_WEIGHT: f64 = 5.0;
const RECOMMENDATION_SCORE_BASELINE_TAG_WEIGHT: f64 = 7.0;
const RECOMMENDATION_SCORE_PROFILE_TAG_WEIGHT: f64 = 4.0;
const SEGMENTED_RECOMMENDATION_TAGS: [&str; 1] = ["anime"];
const MIN_SEGMENTED_RECOMMENDATION_PROFILE_ITEMS: usize = 3;
const MOOD_RECOMMENDATION_TAGS: [&str; 17] = [
    "admiring",
    "adoring",
    "aggressive",
    "amused",
    "audacious",
    "awestruck",
    "bold",
    "cheerful",
    "comforting",
    "complex",
    "dramatic",
    "excited",
    "hilarious",
    "inspirational",
    "intense",
    "nostalgic",
    "playful",
];
const MOOD_RECOMMENDATION_TAG_WEIGHT: f64 = 0.45;
const MAX_TAG_TASTE_ROWS: usize = 4;
const MIN_TAG_TASTE_ROW_ITEMS: usize = 3;
const TAG_TASTE_FETCH_LIMIT_MULTIPLIER: u32 = 16;
const TAG_TASTE_SCORE_WEIGHT: f64 = 6.0;
const BANNED_EXACT_RECOMMENDATION_TAGS: [&str; 21] = [
    "amazon prime video",
    "apple tv",
    "appletv",
    "crunchyroll",
    "disney",
    "disney plus",
    "disney+",
    "fandango at home",
    "fubotv",
    "google play movies",
    "hbo",
    "hbo max",
    "hidive",
    "hulu",
    "magellan",
    "netflix",
    "paramount",
    "peacock",
    "prime",
    "prime video",
    "youtube",
];

fn is_blocked_recommendation_tag(tag: &str) -> bool {
    let normalized = tag
        .trim()
        .to_lowercase();
    if normalized.is_empty() {
        return true;
    }
    if normalized.contains(".tmdb.")
        || normalized.contains(":tmdb")
        || normalized.contains(".imdb.")
        || normalized.contains(":imdb")
        || normalized.contains(".tvdb.")
        || normalized.contains(":tvdb")
    {
        return true;
    }
    if matches!(
        normalized.as_str(),
        "top" | "top rated" | "trending" | "popular"
    ) || BANNED_EXACT_RECOMMENDATION_TAGS.contains(&normalized.as_str()) {
        return true;
    }

    BANNED_RECOMMENDATION_TAGS
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
}

fn has_segmented_recommendation_tag(tags: &[String]) -> bool {
    tags.iter()
        .filter_map(|tag| clean_recommendation_tag(tag))
        .any(|tag| SEGMENTED_RECOMMENDATION_TAGS.contains(&tag.as_str()))
}

fn external_id_looks_segmented(id: &str) -> bool {
    let normalized = id
        .trim()
        .to_lowercase();
    normalized.contains("kitsu")
        || normalized.contains("anilist")
        || normalized.contains("myanimelist")
        || normalized.contains("my-anime-list")
        || normalized.starts_with("mal:")
        || normalized.starts_with("mal.")
}

fn media_has_segmented_identity(media: &db::Media) -> bool {
    has_segmented_recommendation_tag(&media.tags)
        || media
            .external_ids
            .custom_stremio_id
            .as_deref()
            .is_some_and(external_id_looks_segmented)
        || media
            .external_ids
            .series_custom_stremio_id
            .as_deref()
            .is_some_and(external_id_looks_segmented)
}

fn segmented_profile_seed_count(recently_played: &[db::Media], liked: &[db::Media]) -> usize {
    let mut seen_ids = HashSet::new();
    recently_played
        .iter()
        .chain(liked.iter())
        .filter(|media| seen_ids.insert(media.id) && media_has_segmented_identity(media))
        .count()
}

fn passes_segmented_tag_gate(
    profile_tags: &HashSet<String>,
    local_baseline_tags: Option<&HashSet<String>>,
    media_tags: &[String],
) -> bool {
    for tag in media_tags
        .iter()
        .filter_map(|tag| clean_recommendation_tag(tag))
    {
        if SEGMENTED_RECOMMENDATION_TAGS.contains(&tag.as_str())
            && !profile_tags.contains(&tag)
            && !local_baseline_tags.map_or(false, |tags| tags.contains(&tag))
        {
            return false;
        }
    }
    true
}

fn passes_segmented_media_gate(
    profile_tags: &HashSet<String>,
    local_baseline_tags: Option<&HashSet<String>>,
    media: &db::Media,
) -> bool {
    if media_has_segmented_identity(media)
        && !profile_tags
            .iter()
            .any(|tag| SEGMENTED_RECOMMENDATION_TAGS.contains(&tag.as_str()))
        && !local_baseline_tags.map_or(false, |tags| {
            tags.iter()
                .any(|tag| SEGMENTED_RECOMMENDATION_TAGS.contains(&tag.as_str()))
        })
    {
        return false;
    }

    passes_segmented_tag_gate(profile_tags, local_baseline_tags, &media.tags)
}

fn shared_tag_count(baseline_tags: &HashSet<String>, media_tags: &[String]) -> usize {
    let mut count = 0usize;
    let mut seen = HashSet::new();
    for tag in media_tags {
        if let Some(cleaned) = clean_recommendation_tag(tag) {
            if seen.insert(cleaned.clone()) && baseline_tags.contains(&cleaned) {
                count += 1;
            }
        }
    }
    count
}

fn is_mood_recommendation_tag(tag: &str) -> bool {
    MOOD_RECOMMENDATION_TAGS.contains(&tag)
}

fn is_strong_taste_row_tag(tag: &str) -> bool {
    !is_mood_recommendation_tag(tag) && !SEGMENTED_RECOMMENDATION_TAGS.contains(&tag)
}

fn recommendation_tag_weight(tag: &str) -> f64 {
    if is_mood_recommendation_tag(tag) {
        MOOD_RECOMMENDATION_TAG_WEIGHT
    } else {
        1.0
    }
}

fn shared_tag_score(baseline_tags: &HashSet<String>, media_tags: &[String]) -> f64 {
    let mut score = 0.0f64;
    let mut seen = HashSet::new();
    for tag in media_tags {
        if let Some(cleaned) = clean_recommendation_tag(tag) {
            if seen.insert(cleaned.clone()) && baseline_tags.contains(&cleaned) {
                score += recommendation_tag_weight(&cleaned);
            }
        }
    }
    score
}

fn has_meaningful_overlap(
    baseline_tags: &HashSet<String>,
    overlap_genres: usize,
    media_tags: &[String],
) -> bool {
    if overlap_genres == 0 {
        return false;
    }
    let shared_tags = shared_tag_count(baseline_tags, media_tags);
    if baseline_tags.is_empty() {
        return overlap_genres >= 2;
    }
    if overlap_genres >= 4 {
        return shared_tags >= 2;
    }
    if baseline_tags.len() <= 2 {
        return shared_tags >= 2;
    }
    if overlap_genres >= 3 {
        return shared_tags >= 2;
    }
    shared_tags >= 2
}

fn global_tag_overlap_threshold(profile_tags: &HashSet<String>) -> usize {
    if profile_tags.is_empty() {
        1
    } else {
        MIN_GLOBAL_RECOMMENDATION_TAGS
    }
}

fn min_profile_overlap_threshold(tags: &HashSet<String>) -> usize {
    if tags.is_empty() {
        1
    } else if tags.len() <= 4 {
        2
    } else {
        MIN_SHARED_TAGS_FOR_STRONG_MATCH
    }
}

fn min_person_global_overlap_threshold(tag_count: usize) -> usize {
    if tag_count == 0 {
        0
    } else if tag_count <= 2 {
        2
    } else {
        3
    }
}

fn min_profile_overlap_for_profile_tags(profile_tags: &HashSet<String>) -> usize {
    if profile_tags.len() <= 1 {
        1
    } else if profile_tags.len() <= 4 {
        2
    } else {
        3
    }
}

fn min_person_tag_overlap(profile_tag_count: usize, is_actor: bool) -> usize {
    if profile_tag_count <= 1 {
        if is_actor { 3 } else { 1 }
    } else if is_actor {
        if profile_tag_count == 2 { 3 } else { 4 }
    } else {
        2
    }
}

fn min_person_profile_overlap_for_role(
    role: &db::RelationRole,
    person_profile_tag_count: usize,
    baseline_tag_count: usize,
) -> usize {
    match role {
        db::RelationRole::Actor => {
            if baseline_tag_count <= 2 {
                3
            } else if person_profile_tag_count <= 2 {
                3
            } else {
                MIN_PERSON_BASELINE_TAG_OVERLAP_ACTOR
                    .min(person_profile_tag_count)
                    .max(3)
            }
        }
        _ => 2,
    }
}

fn min_person_global_overlap_for_role(
    role: &db::RelationRole,
    baseline_tag_count: usize,
) -> usize {
    match role {
        db::RelationRole::Actor => {
            if baseline_tag_count <= 2 {
                min_person_global_overlap_threshold(baseline_tag_count)
            } else {
                3
            }
        }
        _ => min_person_global_overlap_threshold(baseline_tag_count),
    }
}

fn recommendation_profile_tags<'a, I>(
    tags: I,
    top_n: usize,
    min_occurrences: usize,
) -> HashSet<String>
where
    I: IntoIterator<Item = &'a String>,
{
    let mut counts: HashMap<String, usize> = HashMap::new();
    for tag in tags {
        if let Some(cleaned) = clean_recommendation_tag(tag) {
            *counts
                .entry(cleaned)
                .or_insert(0) += 1;
        }
    }

    let mut ranked: Vec<(String, usize)> = counts
        .into_iter()
        .collect();
    ranked.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| {
                a.0.cmp(&b.0)
            })
    });

    let mut selected = HashSet::new();
    for (tag, count) in &ranked {
        if selected.len() >= top_n {
            break;
        }
        if *count >= min_occurrences {
            selected.insert(tag.clone());
        }
    }

    if selected.is_empty() {
        for (tag, _) in ranked
            .into_iter()
            .take(top_n)
        {
            selected.insert(tag);
        }
    }

    selected
}

fn has_profile_overlap(
    baseline_tags: &HashSet<String>,
    media_tags: &[String],
    min_shared: usize,
) -> bool {
    baseline_tags.is_empty()
        || shared_tag_count(baseline_tags, media_tags) >= min_shared
}

fn build_recommendation_signal(
    signal_type: &str,
    label: &str,
    score: f64,
    role: Option<&str>,
) -> api::RecommendationSignal {
    api::RecommendationSignal {
        type_field: Some(signal_type.to_string()),
        value: Some(label.to_string()),
        label: Some(label.to_string()),
        score: Some(score),
        role: role.map(ToString::to_string),
        displayable: Some(true),
    }
}

fn clean_recommendation_tag(tag: &str) -> Option<String> {
    let cleaned = tag
        .trim()
        .replace('_', " ");
    let normalized = cleaned
        .trim()
        .to_lowercase();
    if is_blocked_recommendation_tag(&normalized) {
        return None;
    }
    Some(normalized)
}

fn append_signals_from_genres(
    genre_ids: &[Uuid],
    genre_names: &HashMap<Uuid, String>,
    signals: &mut Vec<api::RecommendationSignal>,
    limit: usize,
    score_boost: f64,
    role: Option<&str>,
) {
    let mut names: Vec<String> = genre_ids
        .iter()
        .filter_map(|genre_id| genre_names.get(genre_id))
        .cloned()
        .collect();
    names.sort();
    names.dedup();
    for name in names
        .iter()
        .take(limit)
    {
        signals.push(build_recommendation_signal(
            "Genre",
            name,
            score_boost,
            role,
        ));
    }
}

fn append_signals_from_common_tags(
    baseline_tags: &HashSet<String>,
    media: &db::Media,
    signals: &mut Vec<api::RecommendationSignal>,
    limit: usize,
    score_boost: f64,
) {
    let mut tags: Vec<String> = media
        .tags
        .iter()
        .filter_map(|tag| clean_recommendation_tag(tag))
        .filter(|tag| baseline_tags.contains(tag))
        .collect();
    tags.sort_by(|left, right| {
        let left_mood = is_mood_recommendation_tag(left);
        let right_mood = is_mood_recommendation_tag(right);
        left_mood
            .cmp(&right_mood)
            .then_with(|| left.cmp(right))
    });
    tags.dedup();
    tags.into_iter()
        .take(limit)
        .for_each(|tag| {
            signals.push(build_recommendation_signal(
                "Tag",
                &tag,
                score_boost * recommendation_tag_weight(&tag),
                None,
            ));
        });
}

fn top_overlap_context_tags(
    profile_tags: &HashSet<String>,
    media: &db::Media,
    limit: usize,
) -> Vec<String> {
    let mut tags: Vec<String> = media
        .tags
        .iter()
        .filter_map(|tag| clean_recommendation_tag(tag))
        .filter(|tag| profile_tags.contains(tag))
        .collect();
    tags.sort_by(|left, right| {
        let left_segmented = SEGMENTED_RECOMMENDATION_TAGS.contains(&left.as_str());
        let right_segmented = SEGMENTED_RECOMMENDATION_TAGS.contains(&right.as_str());
        let left_mood = is_mood_recommendation_tag(left);
        let right_mood = is_mood_recommendation_tag(right);
        right_segmented
            .cmp(&left_segmented)
            .then_with(|| left_mood.cmp(&right_mood))
            .then_with(|| left.cmp(right))
    });
    tags.dedup();
    let non_mood_count = tags
        .iter()
        .filter(|tag| !is_mood_recommendation_tag(tag))
        .count();
    if non_mood_count >= limit {
        tags.retain(|tag| !is_mood_recommendation_tag(tag));
    }
    tags.truncate(limit);
    tags
}

fn format_reason_context(context: &[String]) -> String {
    if context.is_empty() {
        return String::new();
    }

    let fragment = context
        .iter()
        .take(2)
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(", especially {fragment}")
}

fn format_profile_traits(primary: Option<&str>, context: &[String]) -> String {
    let mut traits = Vec::new();
    if let Some(primary) = primary.filter(|value| !value.is_empty()) {
        traits.push(primary.to_string());
    }
    let primary_key = primary
        .unwrap_or_default()
        .to_ascii_lowercase();
    for tag in context {
        if tag.to_ascii_lowercase() != primary_key {
            traits.push(tag.clone());
        }
    }

    traits.dedup();
    match traits.as_slice() {
        [] => "your recent taste profile".to_string(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [first, second, ..] => format!("{first}, {second}"),
    }
}

fn taste_profile_reason(primary: Option<&str>, context: &[String]) -> String {
    format!(
        "Because your recent watch profile includes {}",
        format_profile_traits(primary, context)
    )
}

fn popular_taste_reason(label: &str, context: &[String]) -> String {
    format!(
        "Because these {label} match your {} taste",
        format_profile_traits(None, context)
    )
}

fn recent_taste_reason(label: &str, context: &[String]) -> String {
    format!(
        "Because these {label} match your {} taste",
        format_profile_traits(None, context)
    )
}

fn reason_for_recommendation_type(
    rec_type: api::RecommendationType,
    baseline_name: &str,
    context: &[String],
) -> String {
    let context_fragment = format_reason_context(context);

    match rec_type {
        api::RecommendationType::SimilarToLikedItem => {
            if baseline_name.is_empty() {
                "Because you like titles with a similar profile".to_string()
            } else if context_fragment.is_empty() {
                format!("Because you liked {baseline_name}")
            } else {
                format!("Because you liked {baseline_name}{context_fragment}")
            }
        }
        api::RecommendationType::HasDirectorFromRecentlyPlayed => {
            if baseline_name.is_empty() {
                "Because this matches your recent viewing profile".to_string()
            } else {
                if context_fragment.is_empty() {
                    format!(
                        "Because this matches your recent viewing profile and includes {baseline_name}"
                    )
                } else {
                    format!(
                        "Because this matches your recent viewing profile and includes {baseline_name}{context_fragment}"
                    )
                }
            }
        }
        api::RecommendationType::HasActorFromRecentlyPlayed => {
            if baseline_name.is_empty() {
                "Because this matches your recent viewing profile".to_string()
            } else {
                if context_fragment.is_empty() {
                    format!(
                        "Because this matches your recent viewing profile and features {baseline_name}"
                    )
                } else {
                    format!(
                        "Because this matches your recent viewing profile and features {baseline_name}{context_fragment}"
                    )
                }
            }
        }
        api::RecommendationType::HasLikedDirector => {
            if baseline_name.is_empty() {
                "Because you like these kinds of directors".to_string()
            } else {
                if context_fragment.is_empty() {
                    format!("Because you follow director {baseline_name}")
                } else {
                    format!(
                        "Because you follow director {baseline_name}, especially {context_fragment}"
                    )
                }
            }
        }
        api::RecommendationType::HasLikedActor => {
            if baseline_name.is_empty() {
                "Because you follow performers in your favorites".to_string()
            } else {
                if context_fragment.is_empty() {
                    format!("Because you follow {baseline_name}")
                } else {
                    format!(
                        "Because you follow {baseline_name}, especially {context_fragment}"
                    )
                }
            }
        }
        api::RecommendationType::SimilarToRecentlyPlayed => {
            if baseline_name.is_empty() {
                "Because it matches your recent taste profile".to_string()
            } else if context_fragment.is_empty() {
                format!("Because you recently watched {baseline_name}")
            } else {
                format!(
                    "Because you recently watched {baseline_name}{context_fragment}"
                )
            }
        }
    }
}

async fn genre_names_for_ids(
    db: &sqlx::SqlitePool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let genres = db::Media::get_by_filter(
        db,
        &db::MediaFilter {
            id: Some(ids.to_vec()),
            kind: Some(vec![db::MediaKind::Genre, db::MediaKind::MusicGenre]),
            limit: Some(ids.len() as u32),
            total_count: false,
            ..Default::default()
        },
    )
    .await?
    .records;

    Ok(genres
        .into_iter()
        .map(|media| (media.id, media.title))
        .collect())
}

async fn overlapping_genres_by_item(
    db: &sqlx::SqlitePool,
    media_ids: &[Uuid],
    baseline_genre_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<Uuid>>> {
    if media_ids.is_empty() || baseline_genre_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut qb = QueryBuilder::new(
        "SELECT mr.left_media_id, mr.right_media_id FROM media_relations mr WHERE mr.left_media_id IN (",
    );
    let mut media_sep = qb.separated(", ");
    for media_id in media_ids {
        media_sep.push_bind(*media_id);
    }
    qb.push(") AND mr.right_media_id IN (");
    let mut genre_sep = qb.separated(", ");
    for genre_id in baseline_genre_ids {
        genre_sep.push_bind(*genre_id);
    }
    qb.push(")");

    let overlaps: Vec<(Uuid, Uuid)> = qb
        .build_query_as()
        .fetch_all(db)
        .await?;

    let mut grouped: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (media_id, genre_id) in overlaps {
        grouped
            .entry(media_id)
            .or_default()
            .push(genre_id);
    }

    Ok(grouped)
}

fn recommendation_title_key(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn filter_categories_by_min_items(
    categories: Vec<api::RecommendationDto>,
    min_items: usize,
) -> Vec<api::RecommendationDto> {
    categories
        .into_iter()
        .filter(|category| category.items.len() >= min_items)
        .collect()
}

fn split_thin_title_categories(
    categories: Vec<api::RecommendationDto>,
    min_items: usize,
) -> (Vec<api::RecommendationDto>, Vec<api::BaseItemDto>, Vec<String>) {
    let mut strong = Vec::new();
    let mut merged_items = Vec::new();
    let mut merged_sources = Vec::new();
    let mut segmented_merged_items = Vec::new();
    let mut segmented_merged_sources = Vec::new();

    for category in categories {
        if category.baseline_item_id.is_some() && category.items.len() < min_items {
            let is_segmented = category
                .items
                .iter()
                .any(item_has_segmented_recommendation_signal);
            let (target_items, target_sources) = if is_segmented {
                (&mut segmented_merged_items, &mut segmented_merged_sources)
            } else {
                (&mut merged_items, &mut merged_sources)
            };
            if let Some(name) = category.baseline_item_name.clone() {
                if !target_sources.contains(&name) {
                    target_sources.push(name);
                }
            }
            target_items.extend(category.items);
        } else {
            strong.push(category);
        }
    }

    if merged_items.is_empty() && !segmented_merged_items.is_empty() {
        merged_items = segmented_merged_items;
        merged_sources = segmented_merged_sources;
    }

    (strong, merged_items, merged_sources)
}

fn item_has_segmented_recommendation_signal(item: &api::BaseItemDto) -> bool {
    item
        .remux
        .as_ref()
        .and_then(|remux| remux.recommendation_explanation.as_ref())
        .and_then(|explanation| explanation.signals.as_ref())
        .map(|signals| {
            signals.iter().any(|signal| {
                signal
                    .value
                    .as_deref()
                    .and_then(clean_recommendation_tag)
                    .is_some_and(|tag| SEGMENTED_RECOMMENDATION_TAGS.contains(&tag.as_str()))
            })
        })
        .unwrap_or(false)
}

fn merged_source_label(source_names: &[String], fallback_name: &str) -> String {
    if source_names.is_empty() {
        return fallback_name.to_string();
    }

    let mut label = source_names
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", " );
    if source_names.len() > 3 {
        label.push_str(", and more");
    }
    label
}

fn merged_similarity_category(
    mut items: Vec<api::BaseItemDto>,
    item_limit: u32,
    recommendation_type: api::RecommendationType,
    source_names: &[String],
    fallback_name: &str,
    category_key: &[u8],
) -> Option<api::RecommendationDto> {
    if items.len() < 2 {
        return None;
    }

    items.truncate(item_limit as usize);
    Some(api::RecommendationDto {
        category_id: Some(Uuid::new_v5(&Uuid::NAMESPACE_OID, category_key)),
        recommendation_type,
        baseline_item_name: Some(merged_source_label(source_names, fallback_name)),
        baseline_item_id: None,
        items,
    })
}

fn attach_recommendation_explanation(
    item: &mut api::BaseItemDto,
    reason: &str,
    signals: Vec<api::RecommendationSignal>,
) {
    if let Some(remux) = &mut item.remux {
        remux.recommendation_explanation = Some(api::RecommendationExplanation {
            reason: Some(reason.to_string()),
            signals: Some(signals),
        });
    }
}

async fn build_tag_taste_categories(
    db: &sqlx::SqlitePool,
    user_id: Uuid,
    excluded_title_keys: &HashSet<String>,
    recently_played: &[db::Media],
    liked: &[db::Media],
    baseline_tags: &HashSet<String>,
    allow_segmented_rows: bool,
    kind: db::MediaKind,
    parent_id: Option<Uuid>,
    item_limit: u32,
    max_categories: usize,
    seen_item_ids: &mut HashSet<Uuid>,
) -> Result<Vec<api::RecommendationDto>> {
    if max_categories == 0 || baseline_tags.is_empty() {
        return Ok(Vec::new());
    }

    let mut tag_scores: HashMap<String, usize> = HashMap::new();
    for media in recently_played.iter().chain(liked.iter()) {
        let mut seen_media_tags = HashSet::new();
        for tag in media.tags.iter().filter_map(|value| clean_recommendation_tag(value)) {
            if baseline_tags.contains(&tag)
                && is_strong_taste_row_tag(&tag)
                && seen_media_tags.insert(tag.clone())
            {
                *tag_scores.entry(tag).or_insert(0) += 1;
            }
        }
    }

    let mut profile_tags: Vec<(String, usize)> = tag_scores.into_iter().collect();
    profile_tags.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    profile_tags.truncate(MAX_TAG_TASTE_ROWS.min(max_categories));
    if profile_tags.is_empty() {
        return Ok(Vec::new());
    }

    let fetch_limit = item_limit
        .saturating_mul(TAG_TASTE_FETCH_LIMIT_MULTIPLIER)
        .clamp(48, 128);
    let candidates = db::Media::get_by_filter(
        db,
        &db::MediaFilter {
            kind: Some(vec![kind.clone()]),
            parent_id,
            recursive: parent_id.is_some(),
            user_id: Some(user_id),
            user_state: Some(db::UserMediaStateFilter {
                user_id: Some(user_id),
                played: Some(false),
                ..Default::default()
            }),
            sort_by: vec![api::ItemSortBy::SimilarityScore],
            sort_order: vec![api::SortOrder::Descending],
            limit: Some(fetch_limit),
            total_count: false,
            ..Default::default()
        },
    )
    .await?
    .records;

    let mut categories = Vec::new();
    for (tag, _tag_score) in profile_tags {
        if !allow_segmented_rows && SEGMENTED_RECOMMENDATION_TAGS.contains(&tag.as_str()) {
            continue;
        }

        let mut row_candidates: Vec<_> = candidates
            .iter()
            .enumerate()
            .filter_map(|(index, media)| {
                if excluded_title_keys.contains(&recommendation_title_key(&media.title)) {
                    return None;
                }
                if !passes_segmented_media_gate(baseline_tags, None, media) {
                    return None;
                }
                if seen_item_ids.contains(&media.id) {
                    return None;
                }
                let media_tags: HashSet<String> = media
                    .tags
                    .iter()
                    .filter_map(|value| clean_recommendation_tag(value))
                    .collect();
                if !media_tags.contains(&tag) {
                    return None;
                }

                let shared_tags = shared_tag_score(baseline_tags, &media.tags);
                let confidence = TAG_TASTE_SCORE_WEIGHT
                    + (shared_tags * RECOMMENDATION_SCORE_PROFILE_TAG_WEIGHT);
                let mut item = api::db_media_to_item(media.clone(), false);
                let mut signals = vec![build_recommendation_signal(
                    "Tag",
                    &tag,
                    SIGNAL_SCORE_TAG_MATCH,
                    None,
                )];
                append_signals_from_common_tags(
                    baseline_tags,
                    media,
                    &mut signals,
                    2,
                    SIGNAL_SCORE_TAG_MATCH,
                );
                signals.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let context = top_overlap_context_tags(baseline_tags, media, 2);
                let reason = taste_profile_reason(Some(&tag), &context);
                attach_recommendation_explanation(&mut item, &reason, signals);
                Some((media.id, item, confidence, index))
            })
            .collect();

        row_candidates.sort_by(|left, right| {
            right
                .2
                .partial_cmp(&left.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.3.cmp(&right.3))
        });
        let items: Vec<_> = row_candidates
            .into_iter()
            .filter_map(|(id, item, _confidence, _index)| {
                if seen_item_ids.insert(id) {
                    Some(item)
                } else {
                    None
                }
            })
            .take(item_limit as usize)
            .collect();

        if items.len() >= MIN_TAG_TASTE_ROW_ITEMS {
            categories.push(api::RecommendationDto {
                category_id: Some(Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("remux-taste-tag-{tag}").as_bytes(),
                )),
                recommendation_type: api::RecommendationType::SimilarToRecentlyPlayed,
                baseline_item_name: Some(tag),
                baseline_item_id: None,
                items,
            });
        }

        if categories.len() >= max_categories {
            break;
        }
    }

    Ok(categories)
}

async fn build_taste_similarity_categories(
    db: &sqlx::SqlitePool,
    user_id: Uuid,
    excluded_title_keys: &HashSet<String>,
    recently_played: &[db::Media],
    liked: &[db::Media],
    baseline_tags: &HashSet<String>,
    allow_segmented_genre_rows: bool,
    kind: db::MediaKind,
    parent_id: Option<Uuid>,
    item_limit: u32,
    max_categories: usize,
    seen_item_ids: &mut HashSet<Uuid>,
) -> Result<Vec<api::RecommendationDto>> {
    if max_categories == 0 {
        return Ok(Vec::new());
    }

    let mut genre_scores: HashMap<Uuid, usize> = HashMap::new();
    let fetch_limit = item_limit
        .saturating_mul(8)
        .clamp(24, 64);

    let mut categories = build_tag_taste_categories(
        db,
        user_id,
        excluded_title_keys,
        recently_played,
        liked,
        baseline_tags,
        allow_segmented_genre_rows,
        kind.clone(),
        parent_id,
        item_limit,
        max_categories,
        seen_item_ids,
    )
    .await?;
    if categories.len() >= max_categories {
        return Ok(categories);
    }

    for media in recently_played
        .iter()
        .chain(liked.iter())
    {
        let mut media = media.clone();
        if media
            .relations
            .is_none()
        {
            media
                .load_relations(db)
                .await?;
        }

        if let Some(rels) = media
            .relations
            .as_ref()
        {
            for (_, related) in rels.iter() {
                if matches!(
                    related.kind,
                    db::MediaKind::Genre | db::MediaKind::MusicGenre
                ) {
                    *genre_scores
                        .entry(related.id)
                        .or_insert(0) += 1;
                }
            }
        }
    }

    let mut top_genres: Vec<(Uuid, usize)> = genre_scores
        .into_iter()
        .collect();
    top_genres.sort_by(|a, b| {
        b.1.cmp(&a.1)
    });
    let top_genre_ids: Vec<Uuid> = top_genres
        .into_iter()
        .take(max_categories)
        .map(|(id, _)| id)
        .collect();

    let genre_names = if top_genre_ids.is_empty() {
        HashMap::new()
    } else {
        genre_names_for_ids(db, &top_genre_ids).await?
    };

    for genre_id in top_genre_ids {
        let Some(genre_name) = genre_names
            .get(&genre_id)
            .cloned()
        else {
            continue;
        };
        if !allow_segmented_genre_rows
            && clean_recommendation_tag(&genre_name).is_some_and(|tag| {
                SEGMENTED_RECOMMENDATION_TAGS.contains(&tag.as_str())
            })
        {
            continue;
        }

        let items = db::Media::get_by_filter(
            db,
            &db::MediaFilter {
                kind: Some(vec![kind.clone()]),
                parent_id,
                recursive: parent_id.is_some(),
                genre_ids: Some(vec![genre_id]),
                user_state: Some(db::UserMediaStateFilter {
                    user_id: Some(user_id),
                    played: Some(false),
                    ..Default::default()
                }),
                sort_by: vec![api::ItemSortBy::CommunityRating],
                sort_order: vec![api::SortOrder::Descending],
                limit: Some(fetch_limit),
                total_count: false,
                ..Default::default()
            },
        )
        .await?
        .records;

        if items.is_empty() {
            continue;
        }

        let mut candidate_items: Vec<_> = items
            .into_iter()
            .enumerate()
            .filter_map(|(index, media)| {
                if excluded_title_keys.contains(&recommendation_title_key(&media.title)) {
                    return None;
                }
                if !passes_segmented_media_gate(baseline_tags, None, &media) {
                    return None;
                }
                if seen_item_ids.contains(&media.id) {
                    return None;
                }

                let shared_tags = shared_tag_score(baseline_tags, &media.tags);
                let confidence = RECOMMENDATION_SCORE_GENRE_WEIGHT
                    + (shared_tags * RECOMMENDATION_SCORE_PROFILE_TAG_WEIGHT);
                let mut item = api::db_media_to_item(media.clone(), false);
                let mut signals = Vec::new();
                signals.push(build_recommendation_signal(
                    "Genre",
                    &genre_name,
                    SIGNAL_SCORE_GENRE_MATCH,
                    None,
                ));
                append_signals_from_common_tags(
                    baseline_tags,
                    &media,
                    &mut signals,
                    2,
                    SIGNAL_SCORE_TAG_MATCH,
                );
                signals.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let context = top_overlap_context_tags(baseline_tags, &media, 2);
                let reason = taste_profile_reason(Some(&genre_name), &context);
                attach_recommendation_explanation(&mut item, &reason, signals);
                let row_context = context.first().cloned();
                Some((media.id, item, confidence, index, row_context))
            })
            .collect();
        candidate_items.sort_by(|left, right| {
            right
                .2
                .partial_cmp(&left.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.3.cmp(&right.3))
        });
        let mut row_context_counts: HashMap<String, usize> = HashMap::new();
        let category_items: Vec<_> = candidate_items
            .into_iter()
            .filter_map(|(id, item, _confidence, _index, row_context)| {
                if seen_item_ids.insert(id) {
                    if let Some(row_context) = row_context {
                        *row_context_counts.entry(row_context).or_insert(0) += 1;
                    }
                    Some(item)
                } else {
                    None
                }
            })
            .take(item_limit as usize)
            .collect();

        if !category_items.is_empty() {
            let row_context = row_context_counts
                .into_iter()
                .filter(|(tag, count)| *count >= 2 && is_strong_taste_row_tag(tag))
                .max_by(|left, right| {
                    left.1
                        .cmp(&right.1)
                        .then_with(|| right.0.cmp(&left.0))
                })
                .map(|(tag, _count)| tag);
            let baseline_name = row_context
                .as_ref()
                .map(|tag| format!("{genre_name} + {tag}"))
                .unwrap_or_else(|| genre_name.clone());
            categories.push(api::RecommendationDto {
                category_id: Some(Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("remux-taste-{baseline_name}").as_bytes(),
                )),
                recommendation_type: api::RecommendationType::SimilarToRecentlyPlayed,
                baseline_item_name: Some(baseline_name),
                baseline_item_id: None,
                items: category_items,
            });
        }

        if categories.len() >= max_categories {
            break;
        }
    }

    if categories.len() >= max_categories {
        return Ok(categories);
    }

    let fallback_items: Vec<_> = db::Media::get_by_filter(
        db,
        &db::MediaFilter {
            kind: Some(vec![kind.clone()]),
            parent_id,
            recursive: parent_id.is_some(),
            user_state: Some(db::UserMediaStateFilter {
                user_id: Some(user_id),
                played: Some(false),
                ..Default::default()
            }),
            sort_by: vec![api::ItemSortBy::CommunityRating],
            sort_order: vec![api::SortOrder::Descending],
            limit: Some(fetch_limit),
            total_count: false,
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .collect();

    if fallback_items.is_empty() {
        return Ok(categories);
    }

    let min_overlap = if categories.is_empty() {
        min_profile_overlap_threshold(baseline_tags)
    } else {
        1
    };
    let mut candidate_items: Vec<_> = fallback_items
        .into_iter()
        .enumerate()
        .filter_map(|(index, media)| {
            if excluded_title_keys.contains(&recommendation_title_key(&media.title)) {
                return None;
            }
            if !passes_segmented_media_gate(baseline_tags, None, &media) {
                return None;
            }
            if !has_profile_overlap(baseline_tags, &media.tags, min_overlap.max(1)) {
                return None;
            }
            if seen_item_ids.contains(&media.id) {
                return None;
            }

            let shared_tags = shared_tag_score(baseline_tags, &media.tags);
            let confidence = shared_tags * RECOMMENDATION_SCORE_PROFILE_TAG_WEIGHT;
            let mut item = api::db_media_to_item(media.clone(), false);
            let mut signals = Vec::new();
            append_signals_from_common_tags(
                baseline_tags,
                &media,
                &mut signals,
                2,
                SIGNAL_SCORE_TAG_MATCH,
            );
            let context = top_overlap_context_tags(baseline_tags, &media, 2);
            let reason = match kind {
                db::MediaKind::Series => popular_taste_reason(
                    "popular unwatched shows",
                    &context,
                ),
                _ if categories.is_empty() => taste_profile_reason(None, &context),
                _ => popular_taste_reason("popular unwatched titles", &context),
            };
            attach_recommendation_explanation(&mut item, &reason, signals);
            Some((media.id, item, confidence, index))
        })
        .collect();
    candidate_items.sort_by(|left, right| {
        right
            .2
            .partial_cmp(&left.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.3.cmp(&right.3))
    });
    let items: Vec<_> = candidate_items
        .into_iter()
        .filter_map(|(id, item, _confidence, _index)| {
            if seen_item_ids.insert(id) {
                Some(item)
            } else {
                None
            }
        })
        .take(item_limit as usize)
        .collect();

    if !items.is_empty() {
        categories.push(api::RecommendationDto {
            category_id: Some(Uuid::new_v5(&Uuid::NAMESPACE_OID, b"remux-taste-popular")),
            recommendation_type: api::RecommendationType::SimilarToRecentlyPlayed,
            baseline_item_name: Some(match kind {
                db::MediaKind::Series => "popular unwatched shows matching your taste",
                _ => "popular picks matching your taste",
            }
            .to_string()),
            baseline_item_id: None,
            items,
        });
    }

    if categories.len() < max_categories {
        let recent_items = db::Media::get_by_filter(
            db,
            &db::MediaFilter {
                kind: Some(vec![kind.clone()]),
                parent_id,
                recursive: parent_id.is_some(),
                user_state: Some(db::UserMediaStateFilter {
                    user_id: Some(user_id),
                    played: Some(false),
                    ..Default::default()
                }),
                sort_by: vec![api::ItemSortBy::DateCreated],
                sort_order: vec![api::SortOrder::Descending],
                limit: Some(fetch_limit),
                total_count: false,
                ..Default::default()
            },
        )
        .await?
        .records;

        let recent_items: Vec<_> = recent_items
            .into_iter()
            .filter_map(|media| {
                if excluded_title_keys.contains(&recommendation_title_key(&media.title)) {
                    return None;
                }
                if !passes_segmented_media_gate(baseline_tags, None, &media) {
                    return None;
                }
                if !has_profile_overlap(baseline_tags, &media.tags, min_overlap.max(1)) {
                    return None;
                }
                if !seen_item_ids.insert(media.id) {
                    return None;
                }

                let mut item = api::db_media_to_item(media.clone(), false);
                let mut signals = Vec::new();
                append_signals_from_common_tags(
                    baseline_tags,
                    &media,
                    &mut signals,
                    2,
                    SIGNAL_SCORE_TAG_MATCH,
                );
                let context = top_overlap_context_tags(baseline_tags, &media, 2);
                let reason = match kind {
                    db::MediaKind::Series => recent_taste_reason(
                        "recently added shows",
                        &context,
                    ),
                    _ => recent_taste_reason("recently added titles", &context),
                };
                attach_recommendation_explanation(&mut item, &reason, signals);
                Some(item)
            })
            .take(item_limit as usize)
            .collect();

        if !recent_items.is_empty() {
            categories.push(api::RecommendationDto {
                category_id: Some(Uuid::new_v5(&Uuid::NAMESPACE_OID, b"remux-taste-recent")),
                recommendation_type: api::RecommendationType::SimilarToRecentlyPlayed,
                baseline_item_name: Some(match kind {
                    db::MediaKind::Series => "recently added shows matching your taste",
                    _ => "recently added picks matching your taste",
                }
                .to_string()),
                baseline_item_id: None,
                items: recent_items,
            });
        }
    }

    Ok(categories)
}

async fn gather_recently_consumed(
    db: &sqlx::SqlitePool,
    user_id: Uuid,
    kind: db::MediaKind,
    parent_id: Option<Uuid>,
    limit: usize,
) -> Result<Vec<db::Media>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    if matches!(kind, db::MediaKind::Series) {
        return gather_recently_consumed_series(db, user_id, parent_id, limit).await;
    }

    let mut baselines: Vec<db::Media> = Vec::new();
    let mut consumed_ids: HashSet<Uuid> = HashSet::new();
    let source_limit = (limit * 2).max(10);

    let played = db::Media::get_by_filter(
        db,
        &db::MediaFilter {
            kind: Some(vec![kind.clone()]),
            parent_id,
            recursive: parent_id.is_some(),
            user_state: Some(db::UserMediaStateFilter {
                user_id: Some(user_id),
                played: Some(true),
                ..Default::default()
            }),
            sort_by: vec![api::ItemSortBy::DatePlayed],
            sort_order: vec![api::SortOrder::Descending],
            limit: Some(source_limit as u32),
            total_count: false,
            ..Default::default()
        },
    )
    .await?
    .records;
    let resumable = db::Media::get_by_filter(
        db,
        &db::MediaFilter {
            kind: Some(vec![kind.clone()]),
            parent_id,
            recursive: parent_id.is_some(),
            user_state: Some(db::UserMediaStateFilter {
                user_id: Some(user_id),
                resumable: Some(true),
                ..Default::default()
            }),
            sort_by: vec![api::ItemSortBy::DatePlayed],
            sort_order: vec![api::SortOrder::Descending],
            limit: Some(source_limit as u32),
            total_count: false,
            ..Default::default()
        },
    )
    .await?
    .records;

    let event_pool = {
        let mut event_ids: HashSet<Uuid> = HashSet::new();
        let events = db::WatchHistory::get_by_filter(
            db,
            &db::WatchHistoryFilter {
                user_id: Some(user_id),
                event_type: Some("playback_stop".to_string()),
                limit: Some((source_limit * 3) as u32),
                total_count: false,
                ..Default::default()
            },
        )
        .await?
        .records;

        let mut media_ids: Vec<Uuid> = Vec::new();
        for event in events {
            if event_ids.insert(event.media_id) {
                media_ids.push(event.media_id);
            }
            if media_ids.len() >= source_limit * 2 {
                break;
            }
        }

        if media_ids.is_empty() {
            Vec::new()
        } else {
            let mut media_by_id: HashMap<Uuid, db::Media> = HashMap::new();
            for media in db::Media::get_by_filter(
                db,
                &db::MediaFilter {
                    kind: Some(vec![kind.clone()]),
                    parent_id,
                    recursive: parent_id.is_some(),
                    id: Some(media_ids.clone()),
                    total_count: false,
                    ..Default::default()
                },
            )
            .await?
            .records
            {
                media_by_id.insert(media.id, media);
            }

            let mut ordered = Vec::new();
            for media_id in media_ids {
                if let Some(media) = media_by_id.remove(&media_id) {
                    ordered.push(media);
                    if ordered.len() >= source_limit {
                        break;
                    }
                }
            }
            ordered
        }
    };

    let pools = [played, resumable, event_pool];
    let mut index = 0usize;
    while baselines.len() < limit {
        let mut added = false;
        for pool in &pools {
            if let Some(item) = pool
                .get(index)
                .cloned()
            {
                if consumed_ids.insert(item.id) {
                    baselines.push(item);
                    added = true;
                }
            }
        }
        if !added {
            break;
        }
        index += 1;
    }

    baselines.truncate(limit);

    Ok(baselines)
}

async fn gather_recently_consumed_series(
    db: &sqlx::SqlitePool,
    user_id: Uuid,
    parent_id: Option<Uuid>,
    limit: usize,
) -> Result<Vec<db::Media>> {
    let source_limit = (limit * 4).max(20) as i64;
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT e.grandparent_id \
         FROM ( \
           SELECT media_id, COALESCE(last_played_at, played_at, '1970-01-01 00:00:00') AS activity_at \
           FROM user_media_state \
           WHERE user_id = ? AND (play_count > 0 OR playback_position > 0) \
           UNION ALL \
           SELECT media_id, COALESCE(created_at, '1970-01-01 00:00:00') AS activity_at \
           FROM watch_history \
           WHERE user_id = ? AND event_type = 'playback_stop' \
         ) active \
         JOIN media e ON e.id = active.media_id \
         WHERE e.kind = 'episode' AND e.grandparent_id IS NOT NULL \
         GROUP BY e.grandparent_id \
         ORDER BY MAX(active.activity_at) DESC \
         LIMIT ?",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(source_limit)
    .fetch_all(db)
    .await?;

    let series_ids: Vec<Uuid> = rows
        .iter()
        .map(|(id,)| *id)
        .collect();
    if series_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut series_by_id: HashMap<Uuid, db::Media> = HashMap::new();
    for series in db::Media::get_by_filter(
        db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Series]),
            parent_id,
            recursive: parent_id.is_some(),
            id: Some(series_ids.clone()),
            total_count: false,
            ..Default::default()
        },
    )
    .await?
    .records
    {
        series_by_id.insert(series.id, series);
    }

    let mut ordered = Vec::new();
    for id in series_ids {
        if let Some(series) = series_by_id.remove(&id) {
            ordered.push(series);
            if ordered.len() >= limit {
                break;
            }
        }
    }

    Ok(ordered)
}

async fn build_similar_categories(
    db: &sqlx::SqlitePool,
    user_id: Uuid,
    excluded_title_keys: &HashSet<String>,
    baselines: &[db::Media],
    baseline_tags: &HashSet<String>,
    kind: db::MediaKind,
    parent_id: Option<Uuid>,
    item_limit: u32,
    min_overlap: usize,
    allow_weak_matches: bool,
    min_items: usize,
    rec_type: api::RecommendationType,
    seen_item_ids: &mut HashSet<Uuid>,
) -> Result<Vec<api::RecommendationDto>> {
    let mut cats = Vec::new();
    let started_at = Instant::now();
    let mut scanned_baselines = 0usize;
    let mut relation_load_elapsed_ms = 0u128;
    let mut genre_name_elapsed_ms = 0u128;
    let mut similar_query_elapsed_ms = 0u128;
    let mut overlap_query_elapsed_ms = 0u128;
    let mut record_fetch_elapsed_ms = 0u128;
    let mut candidate_filter_elapsed_ms = 0u128;
    let baseline_scan_limit = similar_baseline_scan_limit(item_limit, baselines.len());
    for baseline in baselines.iter().take(baseline_scan_limit) {
        scanned_baselines += 1;
        let baseline = baseline.clone();
        let mut baseline = baseline;

        let relation_load_started_at = Instant::now();
        if baseline
            .relations
            .is_none()
        {
            baseline
                .load_relations(db)
                .await?;
        }
        relation_load_elapsed_ms += relation_load_started_at.elapsed().as_millis();
        let baseline_tag_set: HashSet<String> = baseline
            .tags
            .iter()
            .filter_map(|tag| clean_recommendation_tag(tag))
            .collect();
        let baseline_is_segmented = media_has_segmented_identity(&baseline);
        let baseline_genre_ids: Vec<Uuid> = baseline
            .relations
            .as_ref()
            .map(|relations| {
                relations
                    .iter()
                    .filter_map(|(_, related)| {
                        if matches!(
                            related.kind,
                            db::MediaKind::Genre | db::MediaKind::MusicGenre
                        ) {
                            Some(related.id)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let baseline_genre_ids = baseline_genre_ids
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if baseline_genre_ids.len() < 2 {
            continue;
        }
        let genre_name_started_at = Instant::now();
        let baseline_genre_names = genre_names_for_ids(db, &baseline_genre_ids).await?;
        genre_name_elapsed_ms += genre_name_started_at.elapsed().as_millis();

        let candidate_fetch_limit = item_limit
            .saturating_mul(6)
            .clamp(24, 48);
        let similar_query_started_at = Instant::now();
        let scored_ids = db::Media::get_similar_by_genres_for_recommendations(
            db,
            &baseline.id,
            &kind,
            &baseline_genre_ids,
            candidate_fetch_limit,
        )
        .await?;
        similar_query_elapsed_ms += similar_query_started_at.elapsed().as_millis();

        if scored_ids.is_empty() {
            continue;
        }
        let min_overlap = min_overlap as i64;
        let scored_overlap: HashMap<Uuid, usize> = scored_ids
            .iter()
            .map(|(id, overlap)| (*id, *overlap as usize))
            .collect();

        let mut ordered_ids = Vec::new();
        let mut seen_candidate_ids = HashSet::new();
        let mut has_strong_overlap = false;

        for (item_id, overlap) in scored_ids.iter() {
            if overlap >= &min_overlap {
                if seen_candidate_ids.insert(*item_id) {
                    ordered_ids.push(*item_id);
                    has_strong_overlap = true;
                }
            }
        }

        if allow_weak_matches {
            for (item_id, overlap) in scored_ids {
                if overlap >= min_overlap {
                    continue;
                }
                if overlap < min_overlap {
                    if seen_candidate_ids.insert(item_id) {
                        ordered_ids.push(item_id);
                    }
                }
                if ordered_ids.len() >= (item_limit as usize) * 2 {
                    break;
                }
            }
        } else if !has_strong_overlap {
            continue;
        }

        if ordered_ids.len() < min_items {
            continue;
        }

        let overlap_query_started_at = Instant::now();
        let overlap_by_item =
            overlapping_genres_by_item(db, &ordered_ids, &baseline_genre_ids).await?;
        overlap_query_elapsed_ms += overlap_query_started_at.elapsed().as_millis();
        let record_limit = std::cmp::min(
            ordered_ids.len(),
            (item_limit as usize)
                .saturating_mul(4)
                .max(32),
        ) as u32;
        let record_fetch_started_at = Instant::now();
        let records = db::Media::get_by_filter(
            db,
            &db::MediaFilter {
                kind: Some(vec![kind.clone()]),
                parent_id,
                recursive: parent_id.is_some(),
                id: Some(ordered_ids.clone()),
                user_state: Some(db::UserMediaStateFilter {
                    user_id: Some(user_id),
                    played: Some(false),
                    ..Default::default()
                }),
                sort_by: vec![api::ItemSortBy::CommunityRating],
                sort_order: vec![api::SortOrder::Descending],
                limit: Some(record_limit),
                total_count: false,
                ..Default::default()
            },
        )
        .await?
        .records;
        record_fetch_elapsed_ms += record_fetch_started_at.elapsed().as_millis();

        let mut media_by_id: HashMap<Uuid, db::Media> = HashMap::new();
        for media in records {
            media_by_id.insert(media.id, media);
        }

        let candidate_filter_started_at = Instant::now();
        let mut candidate_items: Vec<_> = ordered_ids
            .iter()
            .copied()
            .filter_map(|id| media_by_id.get(&id).cloned())
            .take(
                (item_limit as usize)
                    .saturating_mul(4)
                    .max(32),
            )
            .filter_map(|m| {
                if m.id == baseline.id
                    || excluded_title_keys.contains(&recommendation_title_key(&m.title))
                {
                    return None;
                }
                if media_has_segmented_identity(&m) != baseline_is_segmented {
                    return None;
                }
                if !passes_segmented_media_gate(
                    baseline_tags,
                    Some(&baseline_tag_set),
                    &m,
                ) {
                    return None;
                }
                let overlap_ids = overlap_by_item
                    .get(&m.id)
                    .cloned()
                    .unwrap_or_default();
                let baseline_overlap = std::cmp::max(
                    MIN_SIMILAR_ITEMS_PER_BASELINE,
                    min_profile_overlap_for_profile_tags(&baseline_tag_set),
                );
                let baseline_profile_overlap = std::cmp::max(
                    MIN_SIMILAR_ITEMS_PER_BASELINE,
                    global_tag_overlap_threshold(baseline_tags),
                );
                let global_overlap = shared_tag_count(baseline_tags, &m.tags);
                if !baseline_tags.is_empty()
                    && global_overlap < MIN_PERSON_GLOBAL_TAG_OVERLAP
                {
                    return None;
                }
                let overlap_count = overlap_ids.len();
                let genre_overlap_score = scored_overlap
                    .get(&m.id)
                    .copied()
                    .unwrap_or(0) as f64;

                if !has_profile_overlap(&baseline_tag_set, &m.tags, baseline_overlap) {
                    return None;
                }
                if !has_profile_overlap(
                    baseline_tags,
                    &m.tags,
                    baseline_profile_overlap,
                ) {
                    return None;
                }
                if !has_meaningful_overlap(baseline_tags, overlap_count, &m.tags) {
                    return None;
                }
                if overlap_count < min_overlap as usize {
                    return None;
                }
                if !seen_item_ids.insert(m.id) {
                    return None;
                }

                let baseline_tags_overlap =
                    shared_tag_score(&baseline_tag_set, &m.tags);
                let global_tags_overlap =
                    shared_tag_score(baseline_tags, &m.tags);
                let confidence = (baseline_tags_overlap
                    * RECOMMENDATION_SCORE_BASELINE_TAG_WEIGHT)
                    + (global_tags_overlap * RECOMMENDATION_SCORE_PROFILE_TAG_WEIGHT)
                    + (genre_overlap_score * RECOMMENDATION_SCORE_GENRE_WEIGHT);

                let mut item = api::db_media_to_item(m.clone(), false);
                let mut signals = vec![build_recommendation_signal(
                    "Title",
                    &baseline.title,
                    SIGNAL_SCORE_TITLE_MATCH,
                    None,
                )];
                append_signals_from_genres(
                    &overlap_ids,
                    &baseline_genre_names,
                    &mut signals,
                    2,
                    SIGNAL_SCORE_GENRE_MATCH,
                    None,
                );
                append_signals_from_common_tags(
                    &baseline_tag_set,
                    &m,
                    &mut signals,
                    2,
                    SIGNAL_SCORE_TAG_MATCH,
                );
                let context = top_overlap_context_tags(&baseline_tag_set, &m, 2);
                let reason =
                    reason_for_recommendation_type(rec_type, &baseline.title, &context);
                attach_recommendation_explanation(&mut item, &reason, signals);
                Some((item, confidence))
            })
            .collect();
        if candidate_items.len() < TITLE_STANDALONE_FALLBACK_MIN_ITEMS {
            let mut fallback_items: Vec<_> = ordered_ids
                .iter()
                .copied()
                .filter_map(|id| media_by_id.get(&id).cloned())
                .filter_map(|m| {
                    if m.id == baseline.id
                        || excluded_title_keys.contains(&recommendation_title_key(&m.title))
                    {
                        return None;
                    }
                    if media_has_segmented_identity(&m) != baseline_is_segmented {
                        return None;
                    }
                    if !passes_segmented_media_gate(
                        baseline_tags,
                        Some(&baseline_tag_set),
                        &m,
                    ) {
                        return None;
                    }

                    let overlap_ids = overlap_by_item
                        .get(&m.id)
                        .cloned()
                        .unwrap_or_default();
                    let overlap_count = overlap_ids.len();
                    if overlap_count < min_overlap as usize {
                        return None;
                    }

                        let baseline_tags_overlap_count = shared_tag_count(&baseline_tag_set, &m.tags);
                    let global_tags_overlap_count = shared_tag_count(baseline_tags, &m.tags);
                    if baseline_tags_overlap_count == 0
                        && global_tags_overlap_count == 0
                        && overlap_count < (min_overlap as usize).saturating_add(1)
                    {
                        return None;
                    }
                    if !seen_item_ids.insert(m.id) {
                        return None;
                    }

                    let genre_overlap_score = scored_overlap
                        .get(&m.id)
                        .copied()
                        .unwrap_or(0) as f64;
                    let baseline_tags_overlap = shared_tag_score(&baseline_tag_set, &m.tags);
                    let global_tags_overlap = shared_tag_score(baseline_tags, &m.tags);
                    let confidence = (baseline_tags_overlap
                        * RECOMMENDATION_SCORE_BASELINE_TAG_WEIGHT)
                        + (global_tags_overlap * RECOMMENDATION_SCORE_PROFILE_TAG_WEIGHT)
                        + (genre_overlap_score * RECOMMENDATION_SCORE_GENRE_WEIGHT);

                    let mut item = api::db_media_to_item(m.clone(), false);
                    let mut signals = vec![build_recommendation_signal(
                        "Title",
                        &baseline.title,
                        SIGNAL_SCORE_TITLE_MATCH,
                        None,
                    )];
                    append_signals_from_genres(
                        &overlap_ids,
                        &baseline_genre_names,
                        &mut signals,
                        2,
                        SIGNAL_SCORE_GENRE_MATCH,
                        None,
                    );
                    append_signals_from_common_tags(
                        &baseline_tag_set,
                        &m,
                        &mut signals,
                        1,
                        SIGNAL_SCORE_TAG_MATCH,
                    );
                    let context = top_overlap_context_tags(&baseline_tag_set, &m, 2);
                    let reason =
                        reason_for_recommendation_type(rec_type, &baseline.title, &context);
                    attach_recommendation_explanation(&mut item, &reason, signals);
                    Some((item, confidence))
                })
                .collect();
            candidate_items.append(&mut fallback_items);
        }
        candidate_filter_elapsed_ms += candidate_filter_started_at.elapsed().as_millis();

        candidate_items.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let take_limit = item_limit as usize;
        let mut items: Vec<_> = candidate_items
            .into_iter()
            .map(|(item, _)| item)
            .take(take_limit)
            .collect();
        if items.len() < min_items {
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
    tracing::debug!(
        target: "remux_server::recommendations",
        kind = ?kind,
        rec_type = ?rec_type,
        baseline_count = baselines.len(),
        baseline_scan_limit,
        scanned_baselines,
        returned_categories = cats.len(),
        relation_load_elapsed_ms,
        genre_name_elapsed_ms,
        similar_query_elapsed_ms,
        overlap_query_elapsed_ms,
        record_fetch_elapsed_ms,
        candidate_filter_elapsed_ms,
        total_elapsed_ms = started_at.elapsed().as_millis(),
        "similar recommendation stages"
    );
    Ok(cats)
}

fn similar_baseline_scan_limit(item_limit: u32, baseline_count: usize) -> usize {
    let scan_limit = if item_limit <= 5 { 4 } else { 5 };
    baseline_count.min(scan_limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag_set(values: &[&str]) -> HashSet<String> {
        values
            .iter()
            .filter_map(|value| clean_recommendation_tag(value))
            .collect()
    }

    #[test]
    fn shared_tag_count_handles_noise_and_duplicates() {
        let baseline = tag_set(&["Action", "Drama", "tmdb:Action", "Action"]);
        let media_tags = vec![
            "action".to_string(),
            "drama".to_string(),
            "tmdb:Action".to_string(),
        ];
        assert_eq!(shared_tag_count(&baseline, &media_tags), 2);
    }

    #[test]
    fn has_meaningful_overlap_prefers_tag_matches_when_overlap_is_weak() {
        let baseline_tags = tag_set(&["action", "space", "romance"]);
        assert!(!has_meaningful_overlap(
            &baseline_tags,
            3,
            &["space".to_string(), "comedy".to_string()]
        ));
        assert!(has_meaningful_overlap(
            &baseline_tags,
            3,
            &["space".to_string(), "romance".to_string()]
        ));
        assert!(!has_meaningful_overlap(
            &baseline_tags,
            2,
            &["comedy".to_string(), "thriller".to_string()]
        ));
        assert!(has_meaningful_overlap(
            &HashSet::new(),
            3,
            &["any".to_string()]
        ));
        assert!(has_meaningful_overlap(
            &HashSet::new(),
            2,
            &["any".to_string()]
        ));
    }

    #[test]
    fn recommendation_profile_tags_uses_frequent_tags_first() {
        let raw = vec![
            "action".to_string(),
            "space".to_string(),
            "space".to_string(),
            "drama".to_string(),
            "space".to_string(),
            "drama".to_string(),
            "oneoff".to_string(),
            "tmdb:Action".to_string(),
        ];
        let tags = recommendation_profile_tags(raw.iter(), 4, 2);
        assert!(tags.contains("space"));
        assert!(tags.contains("drama"));
        assert!(!tags.contains("action"));
        assert!(!tags.contains("oneoff"));
        assert!(!tags.contains("tmdb:action"));

        let provider_tag =
            "18cd8af7-26ff-4892-845f-60c2fdb25a18:series:003e3b0.tmdb.top rated";
        assert_eq!(clean_recommendation_tag(provider_tag), None);
        assert_eq!(clean_recommendation_tag("hulu"), None);

        let anime_media_tags = vec!["anime".to_string(), "adventure".to_string()];
        let anime_profile = tag_set(&["anime", "adventure"]);
        let adventure_profile = tag_set(&["adventure"]);
        assert!(passes_segmented_tag_gate(&anime_profile, None, &anime_media_tags));
        assert!(!passes_segmented_tag_gate(
            &adventure_profile,
            None,
            &anime_media_tags
        ));
        assert!(passes_segmented_tag_gate(
            &adventure_profile,
            Some(&anime_profile),
            &anime_media_tags
        ));
    }

    #[test]
    fn min_profile_overlap_threshold_scales_by_profile_size() {
        let one = tag_set(&["action"]);
        let few = tag_set(&["action", "comedy", "drama"]);
        let many = tag_set(&["action", "comedy", "drama", "sci-fi", "fantasy", "war"]);
        assert_eq!(min_profile_overlap_threshold(&one), 2);
        assert_eq!(min_profile_overlap_threshold(&few), 2);
        assert_eq!(min_profile_overlap_threshold(&many), 2);
        assert_eq!(min_profile_overlap_for_profile_tags(&many), 3);
    }

    #[test]
    fn min_person_global_overlap_threshold_scales() {
        assert_eq!(min_person_global_overlap_threshold(0), 0);
        assert_eq!(min_person_global_overlap_threshold(1), 2);
        assert_eq!(min_person_global_overlap_threshold(3), 3);
    }

    #[test]
    fn global_tag_overlap_threshold_is_stricter_when_profile_exists() {
        let one = tag_set(&["action"]);
        let none = tag_set(&[]);
        assert_eq!(
            global_tag_overlap_threshold(&one),
            MIN_GLOBAL_RECOMMENDATION_TAGS
        );
        assert_eq!(global_tag_overlap_threshold(&none), 1);
    }

    #[test]
    fn min_person_tag_overlap_gates_actor_signal_by_profile_richness() {
        assert_eq!(min_person_tag_overlap(0, true), 3);
        assert_eq!(min_person_tag_overlap(2, true), 3);
        assert_eq!(min_person_tag_overlap(3, true), 4);
        assert_eq!(min_person_tag_overlap(4, true), 4);
        assert_eq!(min_person_tag_overlap(10, true), 4);
        assert_eq!(min_person_tag_overlap(4, false), 2);
    }

    #[test]
    fn recommendation_reason_includes_genre_context_for_people() {
        let reason = reason_for_recommendation_type(
            api::RecommendationType::HasActorFromRecentlyPlayed,
            "Tom Hardy",
            &["Drama".to_string(), "Action".to_string()],
        );
        assert!(reason.contains("recent viewing profile"));
        assert!(reason.contains("features Tom Hardy"));
        assert!(reason.contains("Drama"));
    }

    #[test]
    fn person_role_overlap_gates_expand_for_actor_with_richer_profiles() {
        let actor_role = db::RelationRole::Actor;
        let director_role = db::RelationRole::Director;
        assert_eq!(min_person_profile_overlap_for_role(&actor_role, 1, 4), 3);
        assert_eq!(min_person_profile_overlap_for_role(&actor_role, 2, 4), 3);
        assert_eq!(min_person_profile_overlap_for_role(&actor_role, 4, 4), 3);
        assert_eq!(min_person_profile_overlap_for_role(&actor_role, 10, 2), 3);
        assert_eq!(min_person_global_overlap_for_role(&director_role, 4), 3);
        assert_eq!(min_person_global_overlap_for_role(&actor_role, 5), 3);
        assert_eq!(min_person_global_overlap_for_role(&actor_role, 2), 2);
    }

    #[test]
    fn dedupe_recommendation_items_removes_item_duplicates() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let first = api::BaseItemDto {
            id: id_a,
            name: Some("Alpha".to_string()),
            ..Default::default()
        };
        let second = api::BaseItemDto {
            id: id_b,
            name: Some("Beta".to_string()),
            ..Default::default()
        };
        let duplicate = api::BaseItemDto {
            id: id_a,
            name: Some("Alpha Duplicate".to_string()),
            ..Default::default()
        };

        let categories = vec![
            api::RecommendationDto {
                category_id: None,
                recommendation_type: api::RecommendationType::SimilarToRecentlyPlayed,
                baseline_item_name: Some("X".to_string()),
                baseline_item_id: None,
                items: vec![first.clone(), second.clone()],
            },
            api::RecommendationDto {
                category_id: None,
                recommendation_type:
                    api::RecommendationType::HasActorFromRecentlyPlayed,
                baseline_item_name: Some("Y".to_string()),
                baseline_item_id: None,
                items: vec![duplicate, second.clone()],
            },
        ];
        let deduped = dedupe_recommendation_items(categories);

        assert_eq!(deduped.len(), 1);
        assert_eq!(
            deduped[0]
                .items
                .len(),
            2
        );
    }

    #[test]
    fn dedupe_recommendation_categories_removes_repeated_rows_for_same_signal() {
        let id_a = Uuid::new_v4();
        let categories = vec![
            api::RecommendationDto {
                category_id: Some(id_a),
                recommendation_type: api::RecommendationType::SimilarToRecentlyPlayed,
                baseline_item_name: Some("Action Classics".to_string()),
                baseline_item_id: None,
                items: vec![],
            },
            api::RecommendationDto {
                category_id: Some(id_a),
                recommendation_type: api::RecommendationType::SimilarToRecentlyPlayed,
                baseline_item_name: Some("Action Classics".to_string()),
                baseline_item_id: None,
                items: vec![],
            },
            api::RecommendationDto {
                category_id: Some(id_a),
                recommendation_type:
                    api::RecommendationType::HasActorFromRecentlyPlayed,
                baseline_item_name: Some("Action Classics".to_string()),
                baseline_item_id: None,
                items: vec![],
            },
            api::RecommendationDto {
                category_id: Some(Uuid::new_v4()),
                recommendation_type: api::RecommendationType::SimilarToRecentlyPlayed,
                baseline_item_name: Some("Action Classics".to_string()),
                baseline_item_id: None,
                items: vec![],
            },
        ];

        let deduped = dedupe_recommendation_categories(categories);

        assert_eq!(deduped.len(), 3);
    }
}

async fn build_person_categories(
    db: &sqlx::SqlitePool,
    user_id: Uuid,
    excluded_title_keys: &HashSet<String>,
    recently_played: &[db::Media],
    liked: &[db::Media],
    baseline_tags: &HashSet<String>,
    kind: db::MediaKind,
    parent_id: Option<Uuid>,
    item_limit: u32,
    role: db::RelationRole,
    rec_type: api::RecommendationType,
    min_appearances: usize,
    max_categories: usize,
    min_items: usize,
    min_profile_overlap: usize,
    min_baseline_profile_overlap: usize,
    min_distinct_genres: usize,
    seen_item_ids: &mut HashSet<Uuid>,
) -> Result<Vec<api::RecommendationDto>> {
    let mut person_counts: HashMap<Uuid, usize> = HashMap::new();
    let mut person_tag_profiles: HashMap<Uuid, HashMap<String, usize>> = HashMap::new();
    let mut person_genre_counts: HashMap<Uuid, HashMap<Uuid, usize>> = HashMap::new();
    let mut person_names: HashMap<Uuid, String> = HashMap::new();

    let mut seed_movies: Vec<&db::Media> = Vec::with_capacity(16);
    for movie in recently_played
        .iter()
        .take(12)
    {
        if !seed_movies
            .iter()
            .any(|seed| seed.id == movie.id)
        {
            seed_movies.push(movie);
        }
    }
    for movie in liked
        .iter()
        .take(8)
    {
        if !seed_movies
            .iter()
            .any(|seed| seed.id == movie.id)
        {
            seed_movies.push(movie);
        }
    }

    let min_profile_count = if matches!(role, db::RelationRole::Actor) {
        4
    } else {
        2
    };

    for movie in seed_movies {
        let mut movie = movie.clone();
        if movie
            .relations
            .is_none()
        {
            movie
                .load_relations(db)
                .await?;
        }
        let Some(rels) = movie
            .relations
            .as_ref()
        else {
            continue;
        };
        let movie_tag_profile: HashSet<String> = movie
            .tags
            .iter()
            .filter_map(|tag| clean_recommendation_tag(tag))
            .collect();

        let baseline_genre_ids: Vec<Uuid> = rels
            .iter()
            .filter_map(|(_, related)| {
                if matches!(
                    related.kind,
                    db::MediaKind::Genre | db::MediaKind::MusicGenre
                ) {
                    Some(related.id)
                } else {
                    None
                }
            })
            .collect();

        for (relation, person) in rels {
            if relation.role != Some(role.clone()) {
                continue;
            }
            if relation
                .weight
                .unwrap_or(0)
                > 2
            {
                continue;
            }
            let person_id = person.id;
            *person_counts
                .entry(person_id)
                .or_insert(0) += 1;
            person_names
                .entry(person_id)
                .or_insert_with(|| {
                    person
                        .title
                        .clone()
                });
            let person_profile = person_tag_profiles
                .entry(person_id)
                .or_default();
            for tag in movie_tag_profile.iter() {
                if !baseline_tags.is_empty() && !baseline_tags.contains(tag) {
                    continue;
                }
                *person_profile
                    .entry(tag.clone())
                    .or_insert(0) += 1;
            }
            if !baseline_genre_ids.is_empty() {
                let genres = person_genre_counts
                    .entry(person_id)
                    .or_default();
                for genre_id in baseline_genre_ids.iter() {
                    *genres
                        .entry(*genre_id)
                        .or_insert(0) += 1;
                }
            }
        }
    }

    let mut person_ids: Vec<(Uuid, usize)> = person_counts
        .into_iter()
        .filter_map(|(person_id, count)| {
            let distinct_genres = person_genre_counts
                .get(&person_id)
                .map_or(0, |genres| genres.len());
            let person_profile_count = person_tag_profiles
                .get(&person_id)
                .map_or(0, |tags| tags.len());
            (count >= min_appearances && distinct_genres >= min_distinct_genres)
                .then_some(())
                .and_then(|_| {
                    (person_profile_count >= min_profile_count)
                        .then_some((person_id, count))
                })
        })
        .collect();
    person_ids.sort_by(|a, b| {
        let (a_id, a_baseline_count) = a;
        let (b_id, b_baseline_count) = b;
        b_baseline_count
            .cmp(a_baseline_count)
            .then_with(|| {
                let a_genre_count = person_genre_counts
                    .get(a_id)
                    .map(|genres| {
                        genres
                            .values()
                            .sum::<usize>()
                    })
                    .unwrap_or(0);
                let b_genre_count = person_genre_counts
                    .get(b_id)
                    .map(|genres| {
                        genres
                            .values()
                            .sum::<usize>()
                    })
                    .unwrap_or(0);
                b_genre_count
                    .cmp(&a_genre_count)
                    .then_with(|| {
                        let a_name = person_names
                            .get(a_id)
                            .map(String::as_str)
                            .unwrap_or("");
                        let b_name = person_names
                            .get(b_id)
                            .map(String::as_str)
                            .unwrap_or("");
                        a_name.cmp(b_name)
                    })
            })
    });

    let mut cats = Vec::new();
    let mut seen_person_ids: HashSet<Uuid> = HashSet::new();
    let mut all_genre_ids: HashSet<Uuid> = HashSet::new();
    let candidate_person_ids: Vec<Uuid> = person_ids
        .iter()
        .take(max_categories)
        .map(|(person_id, _)| *person_id)
        .collect();
    for person_id in &candidate_person_ids {
        if let Some(genre_counts) = person_genre_counts.get(person_id) {
            let mut ids: Vec<_> = genre_counts
                .iter()
                .collect();
            ids.sort_by(|a, b| {
                b.1.cmp(a.1)
            });
            all_genre_ids.extend(
                ids.into_iter()
                    .take(2)
                    .map(|(id, _)| *id),
            );
        }
    }
    let all_genre_names = genre_names_for_ids(
        db,
        &all_genre_ids
            .into_iter()
            .collect::<Vec<_>>(),
    )
    .await?;

    for (person_id, _) in person_ids
        .into_iter()
        .take(max_categories)
    {
        if !seen_person_ids.insert(person_id) {
            continue;
        }
        let Some(person_name) = person_names.get(&person_id) else {
            continue;
        };
        let mut person_tag_profile: HashSet<String> = person_tag_profiles
            .get(&person_id)
            .map(|tag_counts| {
                let mut tags: Vec<(String, usize)> = tag_counts
                    .iter()
                    .map(|(tag, count)| (tag.clone(), *count))
                    .collect();
                tags.sort_by(|a, b| {
                    b.1.cmp(&a.1)
                        .then_with(|| {
                            a.0.cmp(&b.0)
                        })
                });
                tags.into_iter()
                    .filter(|(_, count)| *count >= 2)
                    .take(6)
                    .map(|(tag, _count)| tag)
                    .collect()
            })
            .unwrap_or_default();
        if person_tag_profile.len() < 2 {
            continue;
        }
        if person_tag_profile.is_empty() {
            continue;
        }

        let genre_ids = person_genre_counts
            .get(&person_id)
            .map(|genre_counts| {
                let mut genre_ids: Vec<_> = genre_counts
                    .iter()
                    .collect();
                genre_ids.sort_by(|a, b| {
                    b.1.cmp(a.1)
                });
                genre_ids
                    .into_iter()
                    .take(2)
                    .map(|(id, _count)| *id)
                    .collect::<Vec<_>>()
            });

        let person_genre_names = genre_ids
            .as_ref()
            .map(|genre_ids| {
                genre_ids
                    .iter()
                    .filter_map(|id| all_genre_names.get(id))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if person_genre_names.is_empty() {
            continue;
        }

        let candidate_fetch_limit = item_limit
            .saturating_mul(4)
            .max(24);
        let items: Vec<_> = db::Media::get_by_filter(
            db,
            &db::MediaFilter {
                kind: Some(vec![kind.clone()]),
                parent_id,
                recursive: parent_id.is_some(),
                person_ids: Some(vec![person_id]),
                genre_ids,
                user_state: Some(db::UserMediaStateFilter {
                    user_id: Some(user_id),
                    played: Some(false),
                    ..Default::default()
                }),
                sort_by: vec![api::ItemSortBy::CommunityRating],
                sort_order: vec![api::SortOrder::Descending],
                limit: Some(candidate_fetch_limit),
                total_count: false,
                ..Default::default()
            },
        )
        .await?
        .records
        .into_iter()
        .take(candidate_fetch_limit as usize)
        .filter_map(|m| {
            if excluded_title_keys.contains(&recommendation_title_key(&m.title)) {
                return None;
            }
            if !passes_segmented_media_gate(
                baseline_tags,
                Some(&person_tag_profile),
                &m,
            ) {
                return None;
            }
            let person_signal_strength =
                has_profile_overlap(&person_tag_profile, &m.tags, {
                    let base =
                        min_profile_overlap_for_profile_tags(&person_tag_profile)
                            .min(min_profile_overlap);
                    if person_tag_profile.len() <= 1 {
                        2
                    } else {
                        base
                    }
                });
            let person_tag_overlap = shared_tag_count(&person_tag_profile, &m.tags);
            let required_role_profile_overlap = min_person_profile_overlap_for_role(
                &role,
                person_tag_profile.len(),
                baseline_tags.len(),
            );
            if person_tag_overlap < required_role_profile_overlap {
                return None;
            }
            let required_overlap = min_person_tag_overlap(
                person_tag_profile.len(),
                matches!(role, db::RelationRole::Actor),
            );
            if person_tag_overlap < required_overlap {
                return None;
            }
            let required_global_overlap =
                min_person_global_overlap_for_role(&role, baseline_tags.len());
            if shared_tag_count(baseline_tags, &m.tags) < required_global_overlap {
                return None;
            }
            let baseline_signal_strength = if baseline_tags.is_empty() {
                true
            } else {
                shared_tag_count(baseline_tags, &m.tags)
                    >= std::cmp::max(
                        min_baseline_profile_overlap,
                        min_person_global_overlap_threshold(baseline_tags.len()),
                    )
            };
            if !person_signal_strength || !baseline_signal_strength {
                return None;
            }
            if !seen_item_ids.insert(m.id) {
                return None;
            }

            let mut signals = Vec::new();
            for genre_name in person_genre_names
                .iter()
                .filter(|genre_name| !genre_name.is_empty())
                .take(2)
            {
                signals.push(build_recommendation_signal(
                    "Genre",
                    genre_name,
                    SIGNAL_SCORE_GENRE_MATCH,
                    None,
                ));
            }
            append_signals_from_common_tags(
                &person_tag_profile,
                &m,
                &mut signals,
                2,
                SIGNAL_SCORE_TAG_MATCH,
            );
            let context = top_overlap_context_tags(&person_tag_profile, &m, 2);
            let person_overlap_score =
                shared_tag_count(&person_tag_profile, &m.tags) as f64;
            let baseline_overlap_score = if baseline_tags.is_empty() {
                0.0
            } else {
                shared_tag_count(baseline_tags, &m.tags) as f64
            };
            let mut item = api::db_media_to_item(m, false);
            signals.push(build_recommendation_signal(
                "Person",
                person_name,
                SIGNAL_SCORE_PERSON_MATCH,
                Some(if matches!(role, db::RelationRole::Director) {
                    "director"
                } else {
                    "actor"
                }),
            ));
            let reason =
                reason_for_recommendation_type(rec_type, person_name, &context);
            attach_recommendation_explanation(&mut item, &reason, signals);
            let score = (person_overlap_score * 2.0) + baseline_overlap_score;
            Some((item, score))
        })
        .collect::<Vec<_>>();
        let mut items: Vec<_> = items
            .into_iter()
            .collect();
        items.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let items: Vec<_> = items
            .into_iter()
            .map(|(item, _)| item)
            .take(item_limit as usize)
            .collect();

        if items.len() >= min_items {
            cats.push(api::RecommendationDto {
                category_id: Some(Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    person_name.as_bytes(),
                )),
                recommendation_type: rec_type,
                baseline_item_name: Some(person_name.clone()),
                baseline_item_id: None,
                items,
            });
        }
    }

    Ok(cats)
}
