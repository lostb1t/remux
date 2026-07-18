use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use axum_extra::extract::Query as ExtraQuery;
use remux_macros::get;

use crate::{AppState, OptionExt, api, api::items::get_items, db, db::auth};

async fn artists_response(
    state: AppState,
    session: auth::AuthSession,
    mut q: api::GetItemsQuery,
) -> Result<impl IntoResponse> {
    q.include_item_types = Some(vec![api::MediaType::MusicArtist]);
    q.recursive = true;
    let result = get_items(state, session, q, true)
        .await?
        .with_client_patches()
        .build();
    Ok(Json(api::BaseItemDtoQueryResult {
        items: result.items,
        total_record_count: result.total_count,
        start_index: 0,
    }))
}

/// `/Artists` — returns all artists in the library.
#[get("/artists")]
pub async fn get_artists(
    State(state): State<AppState>,
    session: auth::AuthSession,
    ExtraQuery(q): ExtraQuery<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    artists_response(state, session, q).await
}

/// `/Artists/AlbumArtists` — same as `/Artists` for our purposes.
#[get("/artists/albumartists")]
pub async fn get_album_artists(
    State(state): State<AppState>,
    session: auth::AuthSession,
    ExtraQuery(q): ExtraQuery<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    artists_response(state, session, q).await
}

/// Jellyfin compatibility lookup for clients that address an artist by name.
#[get("/artists/{name}")]
pub async fn get_artist_by_name(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(name): Path<String>,
) -> Result<impl IntoResponse> {
    let result = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Artist]),
            title_contains: Some(name.clone()),
            limit: Some(50),
            ..Default::default()
        },
    )
    .await?;
    let artist = result
        .records
        .into_iter()
        .find(|item| {
            item.title
                .eq_ignore_ascii_case(&name)
        })
        .context_not_found("Artist not found")?;
    let dto = api::items::item(state, session, artist.id, None)
        .await?
        .context_not_found("Artist not found")?;
    Ok(Json(dto))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use http::header::HeaderValue;

    use super::*;
    use crate::integration_test::{auth_header_with_token, authenticated_server};

    #[tokio::test]
    async fn artist_name_lookup_matches_jellyfin_route() {
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

        let response = server
            .get("/artists/Contract%20Artist")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(&token)).unwrap(),
            )
            .await;
        response.assert_status_ok();
        let body: serde_json::Value = response.json();
        assert_eq!(body["Name"], "Contract Artist");
        assert_eq!(body["Type"], "MusicArtist");
    }
}
