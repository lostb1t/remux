//! Calendar: upcoming and recent release dates as JSON, plus a subscribable
//! ICS feed for external calendar apps.
//!
//! Routes live under the `remux` namespace: this is an additive extension, not
//! part of Jellyfin's API surface.

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveDate, Utc};
use remux_macros::{delete, get, post, query};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    AppState, OptionExt, ResultExt,
    db::{self, auth},
    services::calendar::{self, DateRange},
};
use axum_anyhow::ApiResult as Result;

#[query]
#[derive(Debug, Default)]
pub struct CalendarRangeQuery {
    /// Inclusive range start, `YYYY-MM-DD`. Defaults to the feed window.
    pub from: Option<NaiveDate>,
    /// Inclusive range end, `YYYY-MM-DD`.
    pub to: Option<NaiveDate>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CalendarEventDto {
    pub id: Uuid,
    pub date: NaiveDate,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CalendarResponse {
    pub events: Vec<CalendarEventDto>,
}

/// A user's feed link, including its token.
///
/// Only an administrator can reach the endpoints that return this: the admin
/// creates the link and passes the URL to the person it belongs to.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CalendarLinkDto {
    pub user_id: Uuid,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotated_at: Option<DateTime<Utc>>,
    pub token: String,
    /// Server-relative feed URL to hand to a calendar client.
    pub url: String,
}

impl CalendarLinkDto {
    fn new(link: db::CalendarLink, user_name: String) -> Self {
        let token = link
            .token
            .into_inner();
        Self {
            user_id: link.user_id,
            user_name,
            created_at: link.created_at,
            rotated_at: link.rotated_at,
            url: feed_url(&token),
            token,
        }
    }
}

fn feed_url(token: &str) -> String {
    format!("/remux/calendar/feed/{token}.ics")
}

/// `GET /remux/calendar` — release events visible to the calling user.
#[get("/remux/calendar")]
pub async fn get_calendar(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<CalendarRangeQuery>,
) -> Result<Json<CalendarResponse>> {
    let today = calendar::today();
    let range = match (q.from, q.to) {
        (Some(from), Some(to)) => {
            DateRange::new(from, to).context_bad_request("Invalid calendar range")?
        }
        // A partial range is ambiguous, so fall back to the default window
        // rather than guessing the missing edge.
        _ => DateRange::feed_window(today),
    };

    let events = calendar::events_for_user(&state.ctx, &session.user, range).await?;
    Ok(Json(CalendarResponse {
        events: events
            .into_iter()
            .filter_map(|e| {
                Some(CalendarEventDto {
                    id: e
                        .id
                        .parse()
                        .ok()?,
                    date: e.date,
                    name: e.title,
                    series_name: e.series_title,
                    parent_index_number: e.season_number,
                    index_number: e.episode_number,
                })
            })
            .collect(),
    }))
}

/// `GET /remux/calendar/links` — every issued link, with its token.
///
/// Admin-only: the response is credential material for other users.
#[get("/remux/calendar/links")]
pub async fn list_calendar_links(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<Json<Vec<CalendarLinkDto>>> {
    let links = db::CalendarLink::get_all(
        &state
            .ctx
            .db,
    )
    .await?;
    let names = user_names(
        &state
            .ctx
            .db,
        &links,
    )
    .await?;
    Ok(Json(
        links
            .into_iter()
            .map(|link| {
                let name = names
                    .get(&link.user_id)
                    .cloned()
                    .unwrap_or_default();
                CalendarLinkDto::new(link, name)
            })
            .collect(),
    ))
}

/// `POST /remux/calendar/links/{user_id}` — the user's link, creating one only
/// if they have none.
///
/// Deliberately idempotent: an admin fetching someone's URL to send it must not
/// invalidate the URL that person already subscribed with. Use the rotate
/// endpoint to replace a token on purpose.
#[post("/remux/calendar/links/{user_id}")]
pub async fn create_calendar_link(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(user_id): Path<Uuid>,
) -> Result<Json<CalendarLinkDto>> {
    let user = db::User::get_by_id(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?
    .context_not_found("User not found")?;
    let link = db::CalendarLink::get_or_create(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?;
    Ok(Json(CalendarLinkDto::new(link, user.username)))
}

/// `POST /remux/calendar/links/{user_id}/rotate` — issue a new token, killing
/// the previous URL immediately.
#[post("/remux/calendar/links/{user_id}/rotate")]
pub async fn rotate_calendar_link(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(user_id): Path<Uuid>,
) -> Result<Json<CalendarLinkDto>> {
    let user = db::User::get_by_id(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?
    .context_not_found("User not found")?;
    let link = db::CalendarLink::rotate(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?;
    Ok(Json(CalendarLinkDto::new(link, user.username)))
}

/// `DELETE /remux/calendar/links/{user_id}` — revoke the user's link.
#[delete("/remux/calendar/links/{user_id}")]
pub async fn delete_calendar_link(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode> {
    let deleted = db::CalendarLink::delete_by_user(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?;
    Ok(if deleted {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    })
}

/// Usernames for the owners of `links`, for display alongside each token.
async fn user_names(
    db_pool: &sqlx::SqlitePool,
    links: &[db::CalendarLink],
) -> anyhow::Result<std::collections::HashMap<Uuid, String>> {
    let ids: Vec<Uuid> = links
        .iter()
        .map(|l| l.user_id)
        .collect();
    Ok(db::User::get_by_ids(db_pool, &ids)
        .await?
        .into_iter()
        .map(|u| (u.id, u.username))
        .collect())
}

/// `GET /remux/calendar/feed/{token}.ics` — the subscribable feed.
///
/// Deliberately unauthenticated: calendar clients cannot send Jellyfin auth
/// headers, so the token in the path is the only credential. It is a 256-bit
/// secret, stored hashed, and revocable via `DELETE /remux/calendar/link`.
/// An unknown token returns 404 with no body, so the endpoint reveals nothing
/// about which tokens exist or which users have links.
#[get("/remux/calendar/feed/{token}")]
pub async fn get_calendar_feed(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response> {
    // Subscribers fetch `<token>.ics`; the extension is presentational.
    let token = token
        .strip_suffix(".ics")
        .unwrap_or(&token);

    let Some(link) = db::CalendarLink::get_by_token(
        &state
            .ctx
            .db,
        token,
    )
    .await?
    else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

    // The link outlives the user only until the cascade fires, and a deleted
    // user's feed must stop resolving regardless.
    let Some(user) = db::User::get_by_id(
        &state
            .ctx
            .db,
        &link.user_id,
    )
    .await?
    else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

    let range = DateRange::feed_window(calendar::today());
    let events = calendar::events_for_user(&state.ctx, &user, range).await?;
    let body = calendar::serialize_ics(&events);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/calendar; charset=utf-8")
        .header("Content-Disposition", "inline; filename=\"remux.ics\"")
        // Feed readers poll on their own schedule; caching a stale window would
        // hide newly announced releases.
        .header("Cache-Control", "no-cache, no-store")
        .body(Body::from(body))
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::StatusCode;
    use crate::{
        db,
        integration_test::{auth_header_with_token, authenticated_server},
    };
    use chrono::{Duration, Utc};
    use http::header::HeaderValue;
    use uuid::Uuid;

    /// Inserts a movie premiering `days_from_today` from now.
    async fn insert_movie(
        db_pool: &sqlx::SqlitePool,
        title: &str,
        days_from_today: i64,
    ) -> db::Media {
        let date = Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            + Duration::days(days_from_today);
        // Media::save requires a canonical external ID; derive a stable one from
        // the title so the id is deterministic per test fixture.
        let imdb = db::NonEmptyString::try_new(format!(
            "tt{}",
            title
                .bytes()
                .fold(0_u32, |acc, byte| acc
                    .wrapping_mul(31)
                    .wrapping_add(byte as u32))
        ))
        .unwrap();
        let external_ids = db::ExternalIds {
            imdb: Some(imdb),
            ..Default::default()
        };
        let mut movie = db::Media {
            id: Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Movie,
                external_ids: external_ids.clone(),
                season: None,
                episode: None,
            }),
            title: title.to_string(),
            kind: db::MediaKind::Movie,
            released_at: Some(date),
            external_ids,
            ..Default::default()
        };
        movie
            .save(db_pool)
            .await
            .unwrap();
        movie
    }

    /// Marks `media_id` as favorited by `user_id` — one of the signals that puts
    /// an item in the calendar.
    async fn favorite(db_pool: &sqlx::SqlitePool, user_id: Uuid, media_id: Uuid) {
        db::UserMediaState {
            user_id,
            media_id,
            favorite: true,
            ..Default::default()
        }
        .save(db_pool)
        .await
        .unwrap();
    }

    /// Records in-progress playback, the "continue watching" signal.
    async fn mark_in_progress(
        db_pool: &sqlx::SqlitePool,
        user_id: Uuid,
        media_id: Uuid,
    ) {
        db::UserMediaState {
            user_id,
            media_id,
            playback_position: 300,
            ..Default::default()
        }
        .save(db_pool)
        .await
        .unwrap();
    }

    /// The seeded admin's id.
    async fn admin_id(db_pool: &sqlx::SqlitePool) -> Uuid {
        db::User::get_by_username(db_pool, "test")
            .await
            .unwrap()
            .unwrap()
            .id
    }

    /// Issues (or fetches) `user_id`'s link as an admin and returns its feed URL.
    async fn create_link(
        server: &axum_test::TestServer,
        token: &str,
        user_id: Uuid,
    ) -> String {
        let created: serde_json::Value = server
            .post(&format!("/remux/calendar/links/{user_id}"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(token)).unwrap(),
            )
            .await
            .json();
        created["Url"]
            .as_str()
            .unwrap_or_else(|| panic!("no Url in response: {created}"))
            .to_string()
    }

    /// Creates a link, then fetches its feed with no auth header at all: a
    /// calendar client cannot send one, so the token must be sufficient on its own.
    #[tokio::test]
    async fn feed_is_reachable_with_only_a_token() {
        let (server, guard, token) = authenticated_server().await;
        let db_pool = &guard
            .0
            .db;
        let movie = insert_movie(db_pool, "Feed Movie", 7).await;
        favorite(db_pool, admin_id(db_pool).await, movie.id).await;

        let url = create_link(&server, &token, admin_id(db_pool).await).await;
        let resp = server
            .get(&url)
            .await;
        resp.assert_status_ok();
        assert_eq!(resp.header("content-type"), "text/calendar; charset=utf-8");
        let body = resp.text();
        assert!(body.starts_with("BEGIN:VCALENDAR\r\n"), "{body}");
        assert!(body.contains("SUMMARY:Feed Movie\r\n"), "{body}");
    }

    /// An unknown or revoked token must 404 rather than serve anyone's calendar.
    #[tokio::test]
    async fn feed_rejects_unknown_and_revoked_tokens() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db_pool = &guard
            .0
            .db;
        let uid = admin_id(db_pool).await;

        server
            .get("/remux/calendar/feed/remux_cal_notarealtoken.ics")
            .expect_failure()
            .await
            .assert_status_not_found();

        let url = create_link(&server, &token, uid).await;

        server
            .delete(&format!("/remux/calendar/links/{uid}"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .assert_status(StatusCode::NO_CONTENT);

        // The URL a subscriber already stored must stop working immediately.
        server
            .get(&url)
            .expect_failure()
            .await
            .assert_status_not_found();
    }

    /// Issuing is idempotent so an admin can re-read a URL to pass it on without
    /// breaking the subscription already using it; rotating is the explicit
    /// operation that replaces the token.
    #[tokio::test]
    async fn issuing_is_idempotent_and_rotating_replaces_the_url() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db_pool = &guard
            .0
            .db;
        let uid = admin_id(db_pool).await;

        let first = create_link(&server, &token, uid).await;
        let again = create_link(&server, &token, uid).await;
        assert_eq!(
            first, again,
            "re-issuing must return the existing URL, not invalidate it"
        );

        let rotated: serde_json::Value = server
            .post(&format!("/remux/calendar/links/{uid}/rotate"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let new = rotated["Url"]
            .as_str()
            .unwrap();
        assert_ne!(first.as_str(), new, "rotation must mint a new token");

        server
            .get(&first)
            .expect_failure()
            .await
            .assert_status_not_found();
        server
            .get(new)
            .await
            .assert_status_ok();
    }

    /// A movie released theatrically days ago, with no digital date, must appear:
    /// the availability gate hides those, and a release calendar must not.
    #[tokio::test]
    async fn feed_includes_recent_theatrical_movie_without_digital_date() {
        let (server, guard, token) = authenticated_server().await;
        let db_pool = &guard
            .0
            .db;
        let movie = insert_movie(db_pool, "Theatrical Only", -3).await;
        favorite(db_pool, admin_id(db_pool).await, movie.id).await;

        let url = create_link(&server, &token, admin_id(db_pool).await).await;
        let body = server
            .get(&url)
            .await
            .text();
        assert!(body.contains("SUMMARY:Theatrical Only\r\n"), "{body}");
    }

    /// Items outside the feed window must not leak into it.
    #[tokio::test]
    async fn feed_excludes_items_outside_the_window() {
        let (server, guard, token) = authenticated_server().await;
        let db_pool = &guard
            .0
            .db;
        let uid = admin_id(db_pool).await;
        // All three are favorited, so only the window can exclude them.
        for (title, offset) in [
            ("In Window", 10),
            ("Too Far Ahead", 200),
            ("Too Far Behind", -200),
        ] {
            let movie = insert_movie(db_pool, title, offset).await;
            favorite(db_pool, uid, movie.id).await;
        }

        let url = create_link(&server, &token, admin_id(db_pool).await).await;
        let body = server
            .get(&url)
            .await
            .text();
        assert!(body.contains("SUMMARY:In Window\r\n"), "{body}");
        assert!(!body.contains("Too Far Ahead"), "{body}");
        assert!(!body.contains("Too Far Behind"), "{body}");
    }

    /// Link endpoints return other users' credentials, so a non-admin session
    /// must not reach them — not even for its own link.
    #[tokio::test]
    async fn link_endpoints_are_admin_only() {
        let (server, guard, _admin_token) = authenticated_server().await;
        let db_pool = &guard
            .0
            .db;

        let mut viewer = db::User {
            id: Uuid::new_v4(),
            username: "plain".to_string(),
            is_admin: false,
            ..Default::default()
        };
        viewer
            .set_password("plain")
            .unwrap();
        viewer
            .save(db_pool)
            .await
            .unwrap();

        let auth: serde_json::Value = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(crate::integration_test::AUTH_HEADER),
            )
            .json(&serde_json::json!({ "Username": "plain", "Pw": "plain" }))
            .await
            .json();
        let viewer_auth = auth_header_with_token(
            auth["AccessToken"]
                .as_str()
                .unwrap(),
        );

        let own = viewer.id;
        for (method, path) in [
            ("GET", "/remux/calendar/links".to_string()),
            ("POST", format!("/remux/calendar/links/{own}")),
            ("POST", format!("/remux/calendar/links/{own}/rotate")),
            ("DELETE", format!("/remux/calendar/links/{own}")),
        ] {
            let request = match method {
                "GET" => server.get(&path),
                "POST" => server.post(&path),
                _ => server.delete(&path),
            };
            let status = request
                .add_header(
                    http::header::AUTHORIZATION,
                    HeaderValue::from_str(&viewer_auth).unwrap(),
                )
                .expect_failure()
                .await
                .status_code();
            assert!(
                status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
                "{method} {path} must not be reachable by a non-admin, got {status}"
            );
        }
    }

    /// Each user's feed is their own: a second user's link must not expose the
    /// first user's calendar, and each link must resolve to its owner.
    ///
    /// Isolation is proven with a policy-blocked tag rather than two libraries:
    /// the feed applies the owner's policy, so an item only the admin may see
    /// must be absent from the restricted user's feed while present in the
    /// admin's, from the same library.
    #[tokio::test]
    async fn each_users_feed_applies_only_its_own_owners_policy() {
        let (server, guard, admin_token) = authenticated_server().await;
        let db_pool = &guard
            .0
            .db;

        let restricted = insert_movie(db_pool, "Adults Only", 5).await;
        let open = insert_movie(db_pool, "Everyone", 6).await;
        sqlx::query("INSERT INTO media_tags (media_id, tag) VALUES (?1, ?2)")
            .bind(restricted.id)
            .bind("adult")
            .execute(db_pool)
            .await
            .unwrap();

        // A second user who may not see the tagged item.
        let policy = remux_sdks::remux::UserPolicy {
            blocked_tags: vec!["adult".to_string()],
            ..Default::default()
        };
        let mut viewer = db::User {
            id: Uuid::new_v4(),
            username: "viewer".to_string(),
            is_admin: false,
            policy: Some(sqlx::types::Json(policy)),
            ..Default::default()
        };
        viewer
            .set_password("viewer")
            .unwrap();
        viewer
            .save(db_pool)
            .await
            .unwrap();

        let viewer_auth: serde_json::Value = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(crate::integration_test::AUTH_HEADER),
            )
            .json(&serde_json::json!({ "Username": "viewer", "Pw": "viewer" }))
            .await
            .json();
        let viewer_token = viewer_auth["AccessToken"]
            .as_str()
            .unwrap()
            .to_string();

        // Both users follow both movies, so only policy can differentiate them.
        for user_id in [admin_id(db_pool).await, viewer.id] {
            favorite(db_pool, user_id, restricted.id).await;
            favorite(db_pool, user_id, open.id).await;
        }

        // The admin issues both links; each must still resolve to its own owner.
        let urls = [
            create_link(&server, &admin_token, admin_id(db_pool).await).await,
            create_link(&server, &admin_token, viewer.id).await,
        ];
        assert_ne!(urls[0], urls[1], "each user must get a distinct feed URL");

        let admin_feed = server
            .get(&urls[0])
            .await
            .text();
        let viewer_feed = server
            .get(&urls[1])
            .await
            .text();

        // Both feeds resolve to their own owner, not to whoever asked last.
        assert!(
            admin_feed.contains("SUMMARY:Adults Only\r\n"),
            "{admin_feed}"
        );
        assert!(
            !viewer_feed.contains("Adults Only"),
            "viewer's feed leaked an item their policy blocks: {viewer_feed}"
        );
        // The restricted user still gets their own calendar, not an empty one.
        assert!(
            viewer_feed.contains("SUMMARY:Everyone\r\n"),
            "{viewer_feed}"
        );
    }

    /// The core scoping rule: a Stremio-backed library is mostly noise, so an
    /// untouched item must never appear no matter how imminent its release.
    #[tokio::test]
    async fn feed_excludes_movies_the_user_never_marked() {
        let (server, guard, token) = authenticated_server().await;
        let db_pool = &guard
            .0
            .db;
        let wanted = insert_movie(db_pool, "Wanted Movie", 5).await;
        insert_movie(db_pool, "Library Noise", 6).await;
        favorite(db_pool, admin_id(db_pool).await, wanted.id).await;

        let url = create_link(&server, &token, admin_id(db_pool).await).await;
        let body = server
            .get(&url)
            .await
            .text();
        assert!(body.contains("SUMMARY:Wanted Movie\r\n"), "{body}");
        assert!(
            !body.contains("Library Noise"),
            "an unmarked movie leaked into the calendar: {body}"
        );
    }

    /// Watching a movie is terminal: it implies nothing about a future date, so
    /// playback alone must not qualify a movie. Only an explicit mark does.
    #[tokio::test]
    async fn watching_a_movie_does_not_put_it_in_the_calendar() {
        let (server, guard, token) = authenticated_server().await;
        let db_pool = &guard
            .0
            .db;
        let watched = insert_movie(db_pool, "Already Seen", 4).await;
        mark_in_progress(db_pool, admin_id(db_pool).await, watched.id).await;

        let url = create_link(&server, &token, admin_id(db_pool).await).await;
        let body = server
            .get(&url)
            .await
            .text();
        assert!(
            !body.contains("Already Seen"),
            "a watched movie must not appear: {body}"
        );
    }

    /// Same series-selection criterion as Next Up: playback on ONE episode marks
    /// the whole series as followed, so its unaired siblings appear. State rows
    /// live on episodes, never on the series row, so this exercises the sibling
    /// traversal that a naive `media.id` match would miss.
    #[tokio::test]
    async fn watching_one_episode_surfaces_the_series_upcoming_episodes() {
        use crate::api::shows::test::insert_series_with_episodes;

        let (server, guard, token) = authenticated_server().await;
        let db_pool = &guard
            .0
            .db;

        let (_, watched_eps) = insert_series_with_episodes(
            db_pool,
            "Followed Show",
            &["Seen Episode", "Future Episode"],
        )
        .await;
        let (_, other_eps) = insert_series_with_episodes(
            db_pool,
            "Ignored Show",
            &["Untouched", "Also Untouched"],
        )
        .await;

        let soon = Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            + Duration::days(9);
        // The upcoming episode of each series, only one of which is followed.
        for id in [watched_eps[1].id, other_eps[1].id] {
            sqlx::query(
                "UPDATE media SET released_at = ?1, digital_released_at = NULL WHERE id = ?2",
            )
            .bind(soon)
            .bind(id)
            .execute(db_pool)
            .await
            .unwrap();
        }

        // Watch only the first episode of the first series.
        mark_in_progress(db_pool, admin_id(db_pool).await, watched_eps[0].id).await;

        let url = create_link(&server, &token, admin_id(db_pool).await).await;
        let body = server
            .get(&url)
            .await
            .text();
        assert!(
            body.contains("SUMMARY:Followed Show - S01E02 - Future Episode\r\n"),
            "watching one episode must surface the series' upcoming episode: {body}"
        );
        assert!(
            !body.contains("Also Untouched"),
            "an untouched series leaked into the calendar: {body}"
        );
    }

    /// Favoriting the series row itself must work too: a user may follow a show
    /// they have not started, and its episodes carry no state of their own.
    #[tokio::test]
    async fn favoriting_a_series_surfaces_its_upcoming_episodes() {
        use crate::api::shows::test::insert_series_with_episodes;

        let (server, guard, token) = authenticated_server().await;
        let db_pool = &guard
            .0
            .db;

        let (series, episodes) =
            insert_series_with_episodes(db_pool, "Wishlisted Show", &["Premiere"])
                .await;
        let soon = Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            + Duration::days(12);
        sqlx::query(
            "UPDATE media SET released_at = ?1, digital_released_at = NULL WHERE id = ?2",
        )
        .bind(soon)
        .bind(episodes[0].id)
        .execute(db_pool)
        .await
        .unwrap();

        // Favorite the series, never the episode.
        favorite(db_pool, admin_id(db_pool).await, series.id).await;

        let url = create_link(&server, &token, admin_id(db_pool).await).await;
        let body = server
            .get(&url)
            .await
            .text();
        assert!(
            body.contains("SUMMARY:Wishlisted Show - S01E01 - Premiere\r\n"),
            "favoriting a series must surface its upcoming episodes: {body}"
        );
    }

    /// The window is one continuous interval, so a followed series contributes
    /// its aired, same-day and upcoming episodes alike. Today is the boundary
    /// most likely to be lost to an off-by-one, and unlike /shows/upcoming the
    /// calendar must not restrict itself to the future.
    #[tokio::test]
    async fn feed_includes_past_present_and_future_episodes_of_a_followed_series() {
        use crate::api::shows::test::insert_series_with_episodes;

        let (server, guard, token) = authenticated_server().await;
        let db_pool = &guard
            .0
            .db;

        let (series, episodes) = insert_series_with_episodes(
            db_pool,
            "Airing Show",
            &["Aired Last Week", "Airs Today", "Airs Next Week"],
        )
        .await;

        let today = Utc::now().date_naive();
        let midnight = today
            .and_hms_opt(0, 0, 0)
            .unwrap();
        // Today's episode is stored mid-afternoon: a window ending at 00:00:00
        // rather than end-of-day would silently drop it.
        let air_dates = [
            midnight - Duration::days(7),
            today
                .and_hms_opt(14, 30, 0)
                .unwrap(),
            midnight + Duration::days(7),
        ];
        for (episode, air_date) in episodes
            .iter()
            .zip(air_dates)
        {
            sqlx::query(
                "UPDATE media SET released_at = ?1, digital_released_at = NULL WHERE id = ?2",
            )
            .bind(air_date)
            .bind(episode.id)
            .execute(db_pool)
            .await
            .unwrap();
        }

        favorite(db_pool, admin_id(db_pool).await, series.id).await;

        let url = create_link(&server, &token, admin_id(db_pool).await).await;
        let body = server
            .get(&url)
            .await
            .text();
        for expected in [
            "SUMMARY:Airing Show - S01E01 - Aired Last Week\r\n",
            "SUMMARY:Airing Show - S01E02 - Airs Today\r\n",
            "SUMMARY:Airing Show - S01E03 - Airs Next Week\r\n",
        ] {
            assert!(
                body.contains(expected),
                "missing {expected:?} — the window must cover past, present and future: {body}"
            );
        }
        // Today's episode must land on today's date, not shift a day.
        assert!(
            body.contains(&format!("DTSTART;VALUE=DATE:{}", today.format("%Y%m%d"))),
            "today's episode did not render on today's date: {body}"
        );
    }
}
