use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::{get, post};
use uuid::Uuid;

use crate::{AppState, IntoApiError, OptionExt, api, common, db, db::auth, sdks};
use axum_anyhow::ApiResult as Result;
use chrono::Datelike;

fn lang_three_letter(lang: &str) -> Option<String> {
    use std::str::FromStr;
    let lang = lang
        .trim()
        .to_lowercase();
    isolang::Language::from_639_1(&lang)
        .or_else(|| isolang::Language::from_639_3(&lang))
        .or_else(|| isolang::Language::from_str(&lang).ok())
        .map(|l| {
            l.to_639_3()
                .to_string()
        })
}

fn subtitle_format_from_url(url: &str) -> Option<String> {
    url.rsplit('.')
        .next()
        .and_then(|ext| {
            let low = ext.to_lowercase();
            matches!(low.as_str(), "srt" | "vtt" | "ass" | "ssa" | "sub")
                .then(|| low.to_uppercase())
        })
}

// ── Metadata remote search ──────────────────────────────────────────────────

#[post("/items/remotesearch/movie")]
pub async fn remote_search_movie(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Json(query): Json<api::RemoteSearchQuery>,
) -> Result<impl IntoResponse> {
    let info = query
        .search_info
        .unwrap_or_default();
    let Some(client) = common::tmdb_client(
        &state
            .ctx
            .db,
        &state
            .ctx
            .config
            .tmdb_base_url,
    )
    .await
    else {
        return Ok(Json(Vec::<api::RemoteSearchResult>::new()).into_response());
    };
    let name = info
        .name
        .unwrap_or_default();
    if name.is_empty() {
        return Ok(Json(Vec::<api::RemoteSearchResult>::new()).into_response());
    }
    let resp = client
        .execute(sdks::tmdb::SearchMovieEndpoint {
            query: name,
            year: info.year,
        })
        .await
        .unwrap_or_default();
    let results: Vec<api::RemoteSearchResult> = resp
        .results
        .into_iter()
        .map(|r| {
            let mut provider_ids = std::collections::HashMap::new();
            provider_ids.insert("Tmdb".to_string(), r.id.to_string());
            api::RemoteSearchResult {
                name: Some(r.title),
                production_year: r
                    .release_date
                    .map(|d| d.year() as i64),
                image_url: r
                    .poster_path
                    .map(|p| format!("https://image.tmdb.org/t/p/w500{}", p)),
                search_provider_name: Some("TheMovieDb".to_string()),
                provider_ids,
                premiere_date: r
                    .release_date
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .map(|ndt| {
                        ndt.and_utc()
                            .to_rfc3339()
                    }),
                ..Default::default()
            }
        })
        .collect();
    Ok(Json(results).into_response())
}

#[post("/items/remotesearch/series")]
pub async fn remote_search_series(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Json(query): Json<api::RemoteSearchQuery>,
) -> Result<impl IntoResponse> {
    let info = query
        .search_info
        .unwrap_or_default();
    let Some(client) = common::tmdb_client(
        &state
            .ctx
            .db,
        &state
            .ctx
            .config
            .tmdb_base_url,
    )
    .await
    else {
        return Ok(Json(Vec::<api::RemoteSearchResult>::new()).into_response());
    };
    let name = info
        .name
        .unwrap_or_default();
    if name.is_empty() {
        return Ok(Json(Vec::<api::RemoteSearchResult>::new()).into_response());
    }
    let resp = client
        .execute(sdks::tmdb::SearchTvEndpoint { query: name })
        .await
        .unwrap_or_default();
    let results: Vec<api::RemoteSearchResult> = resp
        .results
        .into_iter()
        .map(|r| {
            let mut provider_ids = std::collections::HashMap::new();
            provider_ids.insert("Tmdb".to_string(), r.id.to_string());
            api::RemoteSearchResult {
                name: Some(r.name),
                production_year: r
                    .first_air_date
                    .map(|d| d.year() as i64),
                image_url: r
                    .poster_path
                    .map(|p| format!("https://image.tmdb.org/t/p/w500{}", p)),
                search_provider_name: Some("TheMovieDb".to_string()),
                provider_ids,
                premiere_date: r
                    .first_air_date
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .map(|ndt| {
                        ndt.and_utc()
                            .to_rfc3339()
                    }),
                ..Default::default()
            }
        })
        .collect();
    Ok(Json(results).into_response())
}

macro_rules! stub_search {
    ($fn_name:ident, $path:literal) => {
        #[post($path)]
        pub async fn $fn_name(
            State(_state): State<AppState>,
            _session: auth::AuthSession,
            Json(_q): Json<api::RemoteSearchQuery>,
        ) -> Result<impl IntoResponse> {
            Ok(Json(Vec::<api::RemoteSearchResult>::new()))
        }
    };
}

stub_search!(remote_search_musicalbum, "/items/remotesearch/musicalbum");
stub_search!(remote_search_musicartist, "/items/remotesearch/musicartist");
stub_search!(remote_search_musicvideo, "/items/remotesearch/musicvideo");
stub_search!(remote_search_person, "/items/remotesearch/person");
stub_search!(remote_search_boxset, "/items/remotesearch/boxset");
stub_search!(remote_search_trailer, "/items/remotesearch/trailer");
stub_search!(remote_search_book, "/items/remotesearch/book");

#[post("/items/remotesearch/apply/{itemid}")]
pub async fn remote_search_apply(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(item_id): Path<Uuid>,
    Json(body): Json<api::ApplySearchResultRequest>,
) -> Result<impl IntoResponse> {
    let mut media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &item_id,
    )
    .await?
    .context_not_found("item not found")?;

    if let Some(ref pids) = body.provider_ids {
        if let Some(s) = pids.get("Tmdb") {
            if let Ok(n) = s.parse::<i64>() {
                media
                    .external_ids
                    .tmdb = Some(n);
            }
        }
        if let Some(s) = pids.get("Imdb") {
            media
                .external_ids
                .imdb = db::NonEmptyString::try_new(s.clone()).ok();
        }
        if let Some(s) = pids.get("Tvdb") {
            if let Ok(n) = s.parse::<i64>() {
                media
                    .external_ids
                    .tvdb = Some(n);
            }
        }
    }

    media
        .save(
            &state
                .ctx
                .db,
        )
        .await
        .map_err(|e| e.context_internal("failed to save item"))?;

    state
        .ctx
        .addons
        .process_meta_batch(vec![media], &state.ctx, true)
        .await
        .map_err(|e| e.context_internal("metadata refresh failed"))?;

    Ok(StatusCode::NO_CONTENT)
}

// ── Subtitle remote search ──────────────────────────────────────────────────

#[remux_macros::query]
#[derive(Debug, Default)]
pub struct SubtitleSearchQuery {
    pub is_perfect_match: Option<bool>,
}

#[get("/items/{itemid}/remotesearch/subtitles/{param}")]
pub async fn search_remote_subtitles(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((item_id, language)): Path<(Uuid, String)>,
    Query(_q): Query<SubtitleSearchQuery>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &item_id,
    )
    .await?
    .context_not_found("item not found")?;

    let subs = state
        .ctx
        .addons
        .fetch_subtitles(
            &media,
            &state
                .ctx
                .db,
            false,
            Some(
                session
                    .user
                    .id,
            ),
        )
        .await;

    let lang_two = crate::api::subtitles::lang_to_two_letter(&language)
        .unwrap_or_else(|| language.to_lowercase());

    let results: Vec<api::RemoteSubtitleInfo> = subs
        .into_iter()
        .filter(|s| {
            s.lang
                .as_deref()
                .and_then(crate::api::subtitles::lang_to_two_letter)
                .map_or(false, |two| two.eq_ignore_ascii_case(&lang_two))
        })
        .map(|s| {
            let id = Uuid::new_v4().to_string();
            let url_str = crate::api::subtitles::descriptor_to_subtitle_url(&s);
            state
                .ctx
                .store
                .save(
                    format!("subtitle:{}", id),
                    url_str,
                    std::time::Duration::from_secs(3600),
                );
            let three_letter = lang_three_letter(
                s.lang
                    .as_deref()
                    .unwrap_or(""),
            );
            let hint = crate::api::subtitles::subtitle_path_hint(&s);
            let format = subtitle_format_from_url(hint);
            api::RemoteSubtitleInfo {
                id,
                name: Some(s.id.clone()),
                provider_name: Some("Stremio".to_string()),
                three_letter_iso_language_name: three_letter,
                format,
                is_hash_match: Some(false),
                ai_translated: Some(false),
                machine_translated: Some(false),
            }
        })
        .collect();

    Ok(Json(results))
}

#[post("/items/{itemid}/remotesearch/subtitles/{param}")]
pub async fn download_remote_subtitle(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((_item_id, subtitle_id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse> {
    let _url: Option<String> = state
        .ctx
        .store
        .get(format!("subtitle:{}", subtitle_id));
    Ok(StatusCode::NO_CONTENT)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use http::{StatusCode, header::HeaderValue};
    use serde_json::json;

    use crate::{
        db,
        integration_test::{auth_header_with_token, authenticated_server},
    };

    #[tokio::test]
    async fn test_remote_search_movie_returns_json_array() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let resp = server
            .post("/items/remotesearch/movie")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "SearchInfo": { "Name": "Inception", "Year": 2010 }
            }))
            .await;

        resp.assert_status_ok();
        let _body: Vec<serde_json::Value> = resp.json();
    }

    #[tokio::test]
    async fn test_remote_search_series_returns_json_array() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let resp = server
            .post("/items/remotesearch/series")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "SearchInfo": { "Name": "Breaking Bad" } }))
            .await;

        resp.assert_status_ok();
        let _body: Vec<serde_json::Value> = resp.json();
    }

    #[tokio::test]
    async fn test_remote_search_stub_routes_return_empty() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let routes = [
            "/items/remotesearch/musicalbum",
            "/items/remotesearch/musicartist",
            "/items/remotesearch/musicvideo",
            "/items/remotesearch/person",
            "/items/remotesearch/boxset",
            "/items/remotesearch/trailer",
            "/items/remotesearch/book",
        ];
        for route in routes {
            let resp = server
                .post(route)
                .add_header(
                    http::header::AUTHORIZATION,
                    HeaderValue::from_str(&auth).unwrap(),
                )
                .json(&json!({ "SearchInfo": { "Name": "Test" } }))
                .await;
            resp.assert_status_ok();
            let body: Vec<serde_json::Value> = resp.json();
            assert!(body.is_empty(), "route {} must return empty array", route);
        }
    }

    #[tokio::test]
    async fn test_remote_search_apply_not_found_returns_404() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let fake_id = uuid::Uuid::new_v4();

        server
            .post(&format!("/items/remotesearch/apply/{}", fake_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .expect_failure()
            .json(&json!({
                "Name": "Inception",
                "ProviderIds": { "Tmdb": "27205" }
            }))
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_remote_search_apply_updates_external_ids() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let now = Utc::now().naive_utc();

        let mut media = db::Media {
            title: "Inception".to_string(),
            kind: db::MediaKind::Stream,
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        media
            .save(
                &guard
                    .0
                    .db,
            )
            .await
            .unwrap();
        let media_id = media.id;

        let resp = server
            .post(&format!("/items/remotesearch/apply/{}", media_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "Name": "Inception",
                "ProviderIds": { "Tmdb": "27205", "Imdb": "tt1375666" }
            }))
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);

        let updated = db::Media::get_by_id(
            &guard
                .0
                .db,
            &media_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            updated
                .external_ids
                .tmdb,
            Some(27205)
        );
        assert_eq!(
            updated
                .external_ids
                .imdb
                .as_ref()
                .map(|s| s.as_str()),
            Some("tt1375666")
        );
    }

    #[tokio::test]
    async fn test_subtitle_search_item_not_found() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let fake_id = uuid::Uuid::new_v4();

        server
            .get(&format!("/items/{}/remotesearch/subtitles/en", fake_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .expect_failure()
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_subtitle_search_no_addons_returns_empty() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let now = Utc::now().naive_utc();

        let mut media = db::Media {
            title: "Test Movie".to_string(),
            kind: db::MediaKind::Stream,
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        media
            .save(
                &guard
                    .0
                    .db,
            )
            .await
            .unwrap();

        let resp = server
            .get(&format!("/items/{}/remotesearch/subtitles/en", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status_ok();
        let body: Vec<serde_json::Value> = resp.json();
        assert!(
            body.is_empty(),
            "no addons configured → must return empty array"
        );
    }

    #[tokio::test]
    async fn test_subtitle_download_returns_204() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let item_id = uuid::Uuid::new_v4();
        let subtitle_id = uuid::Uuid::new_v4();

        let resp = server
            .post(&format!(
                "/items/{}/remotesearch/subtitles/{}",
                item_id, subtitle_id
            ))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);
    }
}
