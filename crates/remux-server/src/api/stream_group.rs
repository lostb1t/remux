use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use http::StatusCode;
use remux_macros::{delete, get, post, put};
use remux_sdks::remux::StreamFilter;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

use crate::{
    AppState,
    db::{ExternalIds, Media, MediaKind, StreamGroup, auth},
};
use axum_anyhow::ApiResult as Result;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamGroupResponse {
    id: Uuid,
    name: String,
    filter: StreamFilter,
    priority: i64,
    enabled: bool,
    hidden: bool,
    created_at: String,
}

impl From<StreamGroup> for StreamGroupResponse {
    fn from(g: StreamGroup) -> Self {
        Self {
            id: g.id,
            name: g.display_name(),
            filter: g.filter,
            priority: g.priority,
            enabled: g.enabled,
            hidden: g.hidden,
            created_at: g.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateStreamGroupPayload {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub filter: StreamFilter,
    #[serde(default)]
    pub priority: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStreamGroupPayload {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub filter: StreamFilter,
    #[serde(default)]
    pub priority: i64,
    #[serde(default = "bool_true")]
    pub enabled: bool,
    #[serde(default)]
    pub hidden: bool,
}

fn bool_true() -> bool {
    true
}

#[get("/remux/stream-groups")]
pub async fn list_stream_groups(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let groups = StreamGroup::list(
        &state
            .ctx
            .db,
    )
    .await?;
    let response: Vec<StreamGroupResponse> = groups
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(response))
}

#[post("/remux/stream-groups")]
pub async fn create_stream_group(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Json(payload): Json<CreateStreamGroupPayload>,
) -> Result<impl IntoResponse> {
    let group = StreamGroup::create(
        &state
            .ctx
            .db,
        &payload.name,
        payload.filter,
        payload.priority,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(StreamGroupResponse::from(group))))
}

#[put("/remux/stream-groups/{id}")]
pub async fn update_stream_group(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateStreamGroupPayload>,
) -> Result<impl IntoResponse> {
    let group = StreamGroup::update(
        &state
            .ctx
            .db,
        &id,
        &payload.name,
        payload.filter,
        payload.priority,
        payload.enabled,
        payload.hidden,
    )
    .await?;
    Ok(Json(StreamGroupResponse::from(group)))
}

#[delete("/remux/stream-groups/{id}")]
pub async fn delete_stream_group(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    StreamGroup::delete(
        &state
            .ctx
            .db,
        &id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct PreviewQuery {
    imdb_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewGroupEntry {
    name: String,
    hidden: bool,
    streams: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewResponse {
    groups: Vec<PreviewGroupEntry>,
    ungrouped: Vec<String>,
}

#[get("/remux/stream-groups/preview")]
pub async fn stream_group_preview(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<PreviewQuery>,
) -> Result<impl IntoResponse> {
    let stub = Media {
        kind: MediaKind::Movie,
        external_ids: ExternalIds {
            imdb: Some(q.imdb_id),
            ..Default::default()
        },
        ..Default::default()
    };

    let raw_streams = state
        .ctx
        .addons
        .get_streams(&stub, &state.ctx)
        .await?;

    let groups = StreamGroup::list(
        &state
            .ctx
            .db,
    )
    .await?;
    let enabled: Vec<&StreamGroup> = groups
        .iter()
        .filter(|g| g.enabled)
        .collect();

    let mut result_groups: Vec<PreviewGroupEntry> = vec![];
    let mut matched_ids: HashSet<Uuid> = HashSet::new();

    for group in &enabled {
        let matching: Vec<&Media> = raw_streams
            .iter()
            .filter(|s| {
                s.stream_info
                    .as_ref()
                    .map_or(false, |info| group.matches(info))
            })
            .collect();

        if matching.is_empty() {
            continue;
        }

        for s in &matching {
            matched_ids.insert(s.id);
        }

        let stream_names: Vec<String> = matching
            .iter()
            .map(|s| {
                s.stream_info
                    .as_ref()
                    .and_then(|i| {
                        i.filename
                            .clone()
                            .or_else(|| {
                                i.name
                                    .clone()
                            })
                    })
                    .unwrap_or_else(|| {
                        s.title
                            .clone()
                    })
            })
            .collect();

        result_groups.push(PreviewGroupEntry {
            name: group.display_name(),
            hidden: group.hidden,
            streams: stream_names,
        });
    }

    let ungrouped: Vec<String> = raw_streams
        .iter()
        .filter(|s| !matched_ids.contains(&s.id))
        .map(|s| {
            s.stream_info
                .as_ref()
                .and_then(|i| {
                    i.filename
                        .clone()
                        .or_else(|| {
                            i.name
                                .clone()
                        })
                })
                .unwrap_or_else(|| {
                    s.title
                        .clone()
                })
        })
        .collect();

    Ok(Json(PreviewResponse {
        groups: result_groups,
        ungrouped,
    }))
}
