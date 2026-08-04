use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_anyhow::ApiResult as Result;
use remux_macros::get;
use uuid::Uuid;

use crate::{AppState, OptionExt, addons::LyricSearchRequest, db, db::auth};

/// `GET /Audio/{item_id}/Lyrics` — fetch the best lyric match for a track.
#[get("/audio/{item_id}/lyrics")]
pub async fn get_lyrics(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(item_id): Path<Uuid>,
) -> Result<Response> {
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &item_id,
    )
    .await?
    .context_not_found("track not found")?;
    if media.kind != db::MediaKind::Track {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }

    let req = build_search_request(
        &state
            .ctx
            .db,
        &media,
    )
    .await;

    let lyrics = state
        .ctx
        .addons
        .lyric_fetch(&req)
        .await?
        .context_not_found("lyrics not found")?;

    Ok(Json(lyrics).into_response())
}

/// `GET /Audio/{item_id}/RemoteSearch/Lyrics` — search all providers for lyrics candidates.
#[get("/audio/{item_id}/remotesearch/lyrics")]
pub async fn search_remote_lyrics(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(item_id): Path<Uuid>,
) -> Result<Response> {
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &item_id,
    )
    .await?
    .context_not_found("track not found")?;
    if media.kind != db::MediaKind::Track {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }

    let req = build_search_request(
        &state
            .ctx
            .db,
        &media,
    )
    .await;
    let results = state
        .ctx
        .addons
        .lyric_search(&req)
        .await?;

    Ok(Json(results).into_response())
}

/// `GET /Providers/Lyrics/{lyric_id}` — fetch a specific lyric by composite ID (e.g. `lrclib_3396226`).
#[get("/providers/lyrics/{lyric_id}")]
pub async fn get_provider_lyrics(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(lyric_id): Path<String>,
) -> Result<Response> {
    let lyrics = state
        .ctx
        .addons
        .lyric_get_by_composite_id(&lyric_id)
        .await?
        .context_not_found("lyrics not found")?;
    Ok(Json(lyrics).into_response())
}

async fn build_search_request(
    db: &sqlx::SqlitePool,
    media: &db::Media,
) -> LyricSearchRequest {
    let (artist, album) = resolve_music_titles(db, media).await;
    LyricSearchRequest {
        title: media
            .title
            .clone(),
        artist,
        album,
        duration_secs: media
            .runtime
            .map(|r| r as f64),
    }
}

pub(crate) async fn resolve_music_titles(
    db: &sqlx::SqlitePool,
    media: &db::Media,
) -> (Option<String>, Option<String>) {
    let ids: Vec<Uuid> = [media.grandparent_id, media.parent_id]
        .into_iter()
        .flatten()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    if ids.is_empty() {
        // Playlist imports have no artist/album rows; the flat names on the
        // track itself are the only source.
        return titles_from_lookup(media, &std::collections::HashMap::new());
    }

    let mut qb = sqlx::QueryBuilder::new("SELECT id, title FROM media WHERE id IN (");
    let mut sep = qb.separated(", ");
    for id in &ids {
        sep.push_bind(id);
    }
    qb.push(")");

    let map: std::collections::HashMap<Uuid, String> = qb
        .build()
        .fetch_all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| {
            use sqlx::Row;
            let id: Option<Uuid> = r.get(0);
            let title: Option<String> = r.get(1);
            id.zip(title)
        })
        .collect();

    titles_from_lookup(media, &map)
}

/// Resolve artist/album names for a track from the loaded parent rows, falling
/// back to the flat `external_ids.artist_name` / `album_title` when the rows
/// are absent (playlist imports).
fn titles_from_lookup(
    media: &db::Media,
    map: &std::collections::HashMap<Uuid, String>,
) -> (Option<String>, Option<String>) {
    let artist = media
        .artist_name_from(
            media
                .grandparent_id
                .and_then(|id| map.get(&id))
                .map(String::as_str),
        )
        .map(str::to_owned);
    let album = media
        .album_name_from(
            media
                .parent_id
                .and_then(|id| map.get(&id))
                .map(String::as_str),
        )
        .map(str::to_owned);
    (artist, album)
}

#[cfg(test)]
mod tests {
    use super::titles_from_lookup;
    use crate::db;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn track(
        grandparent_id: Option<Uuid>,
        parent_id: Option<Uuid>,
        artist_name: Option<&str>,
        album_title: Option<&str>,
    ) -> db::Media {
        db::Media {
            title: "Hello".to_string(),
            grandparent_id,
            parent_id,
            external_ids: db::ExternalIds {
                artist_name: artist_name.map(String::from),
                album_title: album_title.map(String::from),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn uses_artist_row_when_present() {
        let artist_id = Uuid::new_v4();
        let media = track(Some(artist_id), None, Some("Stale"), None);
        let map = HashMap::from([(artist_id, "Adele".to_string())]);
        let (artist, album) = titles_from_lookup(&media, &map);
        assert_eq!(artist.as_deref(), Some("Adele"));
        assert_eq!(album, None);
    }

    #[test]
    fn falls_back_to_flat_artist_name_for_playlist_imports() {
        // Playlist import: no artist row, flat artist_name is the only source.
        let media = track(None, None, Some("Adele"), None);
        let (artist, album) = titles_from_lookup(&media, &HashMap::new());
        assert_eq!(artist.as_deref(), Some("Adele"));
        assert_eq!(album, None);
    }

    #[test]
    fn falls_back_to_flat_album_title_for_playlist_imports() {
        let media = track(None, None, None, Some("21"));
        let (artist, album) = titles_from_lookup(&media, &HashMap::new());
        assert_eq!(artist, None);
        assert_eq!(album.as_deref(), Some("21"));
    }

    #[test]
    fn flat_fallback_only_when_row_missing() {
        let album_id = Uuid::new_v4();
        let media = track(None, Some(album_id), Some("Adele"), Some("Stale"));
        let map = HashMap::from([(album_id, "21".to_string())]);
        let (artist, album) = titles_from_lookup(&media, &map);
        // Artist row missing -> fall back to flat name; album row present -> row wins.
        assert_eq!(artist.as_deref(), Some("Adele"));
        assert_eq!(album.as_deref(), Some("21"));
    }

    #[test]
    fn no_artist_or_album_anywhere() {
        let media = track(None, None, None, None);
        let (artist, album) = titles_from_lookup(&media, &HashMap::new());
        assert_eq!(artist, None);
        assert_eq!(album, None);
    }
}
