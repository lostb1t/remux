use anyhow::Result as AnyResult;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use remux_macros::{get, post};
use serde::Serialize;
use std::collections::HashSet;

use crate::{
    AppState,
    common::tmdb_client,
    db::{self, auth},
    sdks::{self, CachedEndpoint},
    services::image::ImageService,
};
use axum_anyhow::ApiResult as Result;
use uuid::Uuid;

const WATCH_PROVIDERS_CACHE_TTL: std::time::Duration =
    std::time::Duration::from_secs(72 * 60 * 60);

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct StreamMetadataDto {
    pub id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub index: i64,
    pub size: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct StreamsResponse {
    pub streams: Vec<StreamMetadataDto>,
}

fn source_to_dto(s: &db::Media) -> StreamMetadataDto {
    // Title in the DB carries the merged "name\ndescription" string —
    // split it back so legacy clients that only render `Name` still
    // see something useful and ones that consume both get the pair.
    let (name, description) = match s
        .title
        .split_once('\n')
    {
        Some((n, d)) => (
            Some(
                n.trim()
                    .to_string(),
            ),
            Some(
                d.trim()
                    .to_string(),
            ),
        ),
        None if !s
            .title
            .is_empty() =>
        {
            (
                Some(
                    s.title
                        .clone(),
                ),
                None,
            )
        }
        _ => (None, None),
    };
    StreamMetadataDto {
        id: s.id,
        name,
        description,
        index: s
            .idx
            .unwrap_or(0),
        size: s
            .probe_data
            .as_ref()
            .and_then(|p| p.size),
    }
}

/// Source-level metadata (binge group, name, description) for an item.
/// Returns stream metadata from `/gelato/streams/{id}` so the
/// client can match versions across episodes without re-issuing playback info.
async fn streams_metadata(state: &AppState, id: Uuid) -> AnyResult<StreamsResponse> {
    let Some(media) = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    else {
        return Ok(StreamsResponse { streams: vec![] });
    };

    // Accept both the parent (Movie/Episode) and the Source itself.
    let mut parent = if media.kind == db::MediaKind::Stream {
        media
            .parent(
                &state
                    .ctx
                    .db,
            )
            .await?
            .unwrap_or(media)
    } else {
        media
    };

    if !matches!(
        parent.kind,
        db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track
    ) {
        return Ok(StreamsResponse { streams: vec![] });
    }

    let mut sources = parent
        .streams(
            &state
                .ctx
                .db,
        )
        .await
        .unwrap_or_default();
    sources.sort_by_key(|s| {
        s.idx
            .unwrap_or(0)
    });

    let enabled_groups = db::StreamGroup::list_enabled(
        &state
            .ctx
            .db,
    )
    .await
    .unwrap_or_default();

    if enabled_groups.is_empty() {
        let streams = sources
            .iter()
            .map(source_to_dto)
            .collect();
        return Ok(StreamsResponse { streams });
    }

    let config = db::Settings::get_config_or_default(
        &state
            .ctx
            .db,
    )
    .await;
    let show_ungrouped = config
        .stream_groups_show_ungrouped
        .unwrap_or(true);

    let mut result: Vec<StreamMetadataDto> = vec![];
    let mut matched_ids: HashSet<Uuid> = HashSet::new();

    for group in &enabled_groups {
        let matching: Vec<&db::Media> = sources
            .iter()
            .filter(|s| {
                s.stream_info
                    .as_ref()
                    .map_or(false, |info| {
                        group.match_outcome_with_runtime(
                            info,
                            s.probe_data
                                .as_ref(),
                            s.runtime,
                        ) == db::MatchOutcome::Match
                    })
            })
            .collect();

        if matching.is_empty() {
            continue;
        }

        for s in &matching {
            matched_ids.insert(s.id);
        }

        let best = matching[0];
        let description = {
            use remux_sdks::remux::{
                StreamRule, format_bitrate_rule, format_size_rule, language_label,
            };
            let parts: Vec<String> = group
                .filter
                .rules
                .iter()
                .map(|r| match r {
                    StreamRule::Resolution { values, .. } => values
                        .iter()
                        .map(|v| v.label())
                        .collect::<Vec<_>>()
                        .join("/"),
                    StreamRule::Quality { values, .. } => values
                        .iter()
                        .map(|v| v.label())
                        .collect::<Vec<_>>()
                        .join("/"),
                    StreamRule::Codec { values, .. } => values
                        .iter()
                        .map(|v| v.label())
                        .collect::<Vec<_>>()
                        .join("/"),
                    StreamRule::AudioLanguage { values, .. } => values
                        .iter()
                        .map(|c| language_label(c))
                        .collect::<Vec<_>>()
                        .join("/"),
                    StreamRule::Size { op, value } => format_size_rule(*op, *value),
                    StreamRule::Addon { values, .. } => {
                        if values.len() == 1 {
                            "1 addon".to_string()
                        } else {
                            format!("{} addons", values.len())
                        }
                    }
                    StreamRule::SubtitleLanguage { values, .. } => values
                        .iter()
                        .map(|c| format!("{} subs", language_label(c)))
                        .collect::<Vec<_>>()
                        .join("/"),
                    StreamRule::Bitrate { op, value } => {
                        format_bitrate_rule(*op, *value)
                    }
                    StreamRule::Cached { value } => {
                        if *value { "Cached" } else { "Uncached" }.to_string()
                    }
                })
                .filter(|s| !s.is_empty())
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join(" · "))
            }
        };
        result.push(StreamMetadataDto {
            id: best.id,
            name: Some(group.display_name()),
            description,
            index: result.len() as i64,
            size: best
                .probe_data
                .as_ref()
                .and_then(|p| p.size),
        });
    }

    if show_ungrouped {
        for s in &sources {
            if !matched_ids.contains(&s.id) {
                result.push(source_to_dto(s));
            }
        }
    }

    Ok(StreamsResponse { streams: result })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MetricsStatusResponse {
    pub last_updated_days_ago: Option<i64>,
}

#[get("/remux/metrics/status")]
pub async fn remux_metrics_status(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let last_updated_days_ago = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT CAST(julianday('now') - julianday(end_at) AS INTEGER) \
         FROM task_results WHERE task_id = 'RefreshPopularity' \
         AND status = 'completed'",
    )
    .fetch_optional(
        &state
            .ctx
            .db,
    )
    .await?
    .flatten();

    Ok(Json(MetricsStatusResponse {
        last_updated_days_ago,
    }))
}

#[get("/remux/streams/{id}")]
pub async fn remux_streams(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let payload = streams_metadata(&state, id)
        .await
        .unwrap_or(StreamsResponse { streams: vec![] });
    Ok(Json(payload))
}

#[get("/remux/meta/{kind}/{id}")]
pub async fn remux_meta(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((kind, id)): Path<(String, String)>,
) -> Result<impl IntoResponse> {
    let media_type = match kind.as_str() {
        "series" => remux_sdks::stremio::MediaType::Series,
        "movie" => remux_sdks::stremio::MediaType::Movie,
        _ => remux_sdks::stremio::MediaType::Movie,
    };

    let manifest_url = match crate::addons::Addon::list(
        &state
            .ctx
            .db,
    )
    .await
    {
        Ok(addons) => addons
            .into_iter()
            .filter(|a| {
                a.enabled
                    && a.preset
                        .kind
                        == "stremio"
            })
            .find_map(|a| {
                a.preset
                    .config
                    .expose()
                    .get("manifest_url")
                    .and_then(|v| {
                        v.as_str()
                            .map(str::to_string)
                    })
            }),
        Err(_) => None,
    };

    let svc = match manifest_url
        .as_deref()
        .and_then(|u| crate::services::stremio::StremioService::from_url(u).ok())
    {
        Some(a) => a,
        None => {
            return Ok(
                (StatusCode::NOT_FOUND, Json(serde_json::Value::Null)).into_response()
            );
        }
    };

    match svc
        .get_meta(media_type, id)
        .await
    {
        Ok(Some(meta)) => Ok(Json::<remux_sdks::stremio::Meta>(meta).into_response()),
        Ok(None) | Err(_) => {
            Ok((StatusCode::NOT_FOUND, Json(serde_json::Value::Null)).into_response())
        }
    }
}

// ---------------------------------------------------------------------------
// Watch providers
// ---------------------------------------------------------------------------

fn watch_provider_key(name: &str) -> String {
    match name
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "amazon prime video" | "prime video" => "amazon prime video".to_string(),
        "discovery+" | "discovery plus" => "discovery plus".to_string(),
        name => name.to_string(),
    }
}

/// `GET /remux/watch-providers` — list all streaming providers available on TMDB.
#[get("/remux/watch-providers")]
pub async fn get_watch_providers(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<Json<Vec<crate::services::stream_provider::StreamProvider>>> {
    let tmdb_base = state
        .ctx
        .config
        .tmdb_base_url
        .clone();
    let client = tmdb_client(
        &state
            .ctx
            .db,
        &tmdb_base,
    )
    .await
    .ok_or_else(|| anyhow::anyhow!("TMDB client is unavailable"))?;

    let providers = client
        .execute(
            sdks::tmdb::AllMovieWatchProvidersEndpoint {
                language: Some("en-US".to_string()),
                watch_region: None,
            }
            .with_cache(WATCH_PROVIDERS_CACHE_TTL),
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!("failed to fetch TMDB watch providers: {error}")
        })?
        .results;

    let mut dtos: Vec<crate::services::stream_provider::StreamProvider> = providers
        .into_iter()
        .map(|p| {
            crate::services::stream_provider::StreamProvider::new(
                p.provider_id,
                p.provider_name,
                p.logo_path,
            )
        })
        .collect();
    dtos.sort_by_key(|provider| watch_provider_key(&provider.name));
    dtos.dedup_by(|a, b| watch_provider_key(&a.name) == watch_provider_key(&b.name));
    Ok(Json(dtos))
}

// ---------------------------------------------------------------------------
// Collection image regeneration
// ---------------------------------------------------------------------------

/// `POST /remux/collections/{id}/image/regenerate` — delete the cached generated
/// image for a collection so it is rebuilt on the next request.
#[post("/remux/collections/{id}/image/regenerate")]
pub async fn regenerate_collection_image(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    ImageService::invalidate_collection_image(
        &state
            .ctx
            .config
            .data_dir,
        id,
        &state
            .ctx
            .db,
    )
    .await
    .map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}
