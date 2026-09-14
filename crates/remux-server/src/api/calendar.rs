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
    AppState, ResultExt,
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

/// Status of a user's feed link. The token is absent: it is returned only by
/// create and rotate, so a compromised session cannot read back an existing
/// feed URL.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CalendarLinkDto {
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotated_at: Option<DateTime<Utc>>,
}

/// A newly created or rotated link, including its one-time token and feed path.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CalendarCredentialDto {
    pub active: bool,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotated_at: Option<DateTime<Utc>>,
    pub token: String,
    /// Server-relative feed URL to hand to a calendar client.
    pub url: String,
}

impl From<db::CalendarLink> for CalendarLinkDto {
    fn from(link: db::CalendarLink) -> Self {
        Self {
            active: true,
            created_at: Some(link.created_at),
            rotated_at: link.rotated_at,
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

/// `GET /remux/calendar/link` — whether the user has a feed link.
#[get("/remux/calendar/link")]
pub async fn get_calendar_link(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<Json<CalendarLinkDto>> {
    let link = db::CalendarLink::get_by_user(
        &state
            .ctx
            .db,
        &session
            .user
            .id,
    )
    .await?;
    Ok(Json(match link {
        Some(link) => link.into(),
        None => CalendarLinkDto {
            active: false,
            created_at: None,
            rotated_at: None,
        },
    }))
}

/// `POST /remux/calendar/link` — create the user's feed link, or rotate it.
///
/// Rotation is the same operation as creation: it replaces the stored token, so
/// the previous URL stops working immediately.
#[post("/remux/calendar/link")]
pub async fn create_calendar_link(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<Json<CalendarCredentialDto>> {
    let credential = db::CalendarLink::upsert(
        &state
            .ctx
            .db,
        &session
            .user
            .id,
    )
    .await?;
    let token = credential
        .token
        .into_inner();
    Ok(Json(CalendarCredentialDto {
        active: true,
        created_at: credential
            .link
            .created_at,
        rotated_at: credential
            .link
            .rotated_at,
        url: feed_url(&token),
        token,
    }))
}

/// `DELETE /remux/calendar/link` — revoke the user's feed link.
#[delete("/remux/calendar/link")]
pub async fn delete_calendar_link(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<StatusCode> {
    let deleted = db::CalendarLink::delete_by_user(
        &state
            .ctx
            .db,
        &session
            .user
            .id,
    )
    .await?;
    Ok(if deleted {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    })
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

    /// Creates the caller's feed link and returns its URL.
    async fn create_link(server: &axum_test::TestServer, token: &str) -> String {
        let created: serde_json::Value = server
            .post("/remux/calendar/link")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(token)).unwrap(),
            )
            .await
            .json();
        created["Url"]
            .as_str()
            .unwrap()
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

        let url = create_link(&server, &token).await;
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
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        server
            .get("/remux/calendar/feed/remux_cal_notarealtoken.ics")
            .expect_failure()
            .await
            .assert_status_not_found();

        let url = create_link(&server, &token).await;

        server
            .delete("/remux/calendar/link")
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

    /// Rotating replaces the token: the previous URL must die so a shared link
    /// can actually be taken back.
    #[tokio::test]
    async fn rotating_a_link_invalidates_the_previous_url() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let first = create_link(&server, &token).await;
        let second = create_link(&server, &token).await;
        let old = first.as_str();
        let new = second.as_str();
        assert_ne!(old, new, "rotation must mint a new token");

        server
            .get(old)
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

        let url = create_link(&server, &token).await;
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

        let url = create_link(&server, &token).await;
        let body = server
            .get(&url)
            .await
            .text();
        assert!(body.contains("SUMMARY:In Window\r\n"), "{body}");
        assert!(!body.contains("Too Far Ahead"), "{body}");
        assert!(!body.contains("Too Far Behind"), "{body}");
    }

    /// The link endpoint must never hand back an existing token, only its status.
    #[tokio::test]
    async fn link_status_does_not_expose_the_token() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let before: serde_json::Value = server
            .get("/remux/calendar/link")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        assert_eq!(before["Active"], serde_json::json!(false));

        server
            .post("/remux/calendar/link")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        let after: serde_json::Value = server
            .get("/remux/calendar/link")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        assert_eq!(after["Active"], serde_json::json!(true));
        assert!(
            after
                .get("Token")
                .is_none(),
            "link status must not expose the token: {after}"
        );
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

        let urls = [
            create_link(&server, &admin_token).await,
            create_link(&server, &viewer_token).await,
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

        let url = create_link(&server, &token).await;
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

        let url = create_link(&server, &token).await;
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

        let url = create_link(&server, &token).await;
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

        let url = create_link(&server, &token).await;
        let body = server
            .get(&url)
            .await
            .text();
        assert!(
            body.contains("SUMMARY:Wishlisted Show - S01E01 - Premiere\r\n"),
            "favoriting a series must surface its upcoming episodes: {body}"
        );
    }
}
