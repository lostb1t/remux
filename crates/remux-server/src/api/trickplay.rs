//! Jellyfin's trick play sheets, made in the background by [`crate::trickplay`]
//! while a video plays.

use axum::{
    body::Body,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use axum_extra::extract::Query;
use http::{StatusCode, header};
use remux_macros::{get, query};
use uuid::Uuid;

use crate::{
    AppContext, AppState, db, db::auth, ids::UserId, services::StreamService, trickplay,
};

#[query]
pub struct TrickplaySheetQuery {
    pub media_source_id: Option<Uuid>,
}

/// One sheet of 5x5 pictures, as Jellyfin serves it. Answers 503 while the
/// sheet is not made yet, also when no set can be found yet for this device
/// and a film or an episode. Answers 404 for a sheet that can never exist:
/// past the set's picture count, missing from a finished set, of an item that
/// is neither, and whenever the switch is off.
#[get("/videos/{id}/trickplay/{width}/{file}")]
pub async fn trickplay_sheet(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((id, width, file)): Path<(Uuid, u32, String)>,
    Query(q): Query<TrickplaySheetQuery>,
) -> Response {
    let root = trickplay::root_dir(
        &state
            .ctx
            .config
            .data_dir,
    );
    serve_sheet(
        &state.ctx,
        &root,
        SheetRequest {
            user_id: session
                .user
                .id,
            device_id: &session
                .device
                .id,
            id,
            media_source_id: q.media_source_id,
            width,
            file: &file,
        },
    )
    .await
}

/// What a sheet request names, with the caller's user and device.
pub(crate) struct SheetRequest<'a> {
    pub user_id: Uuid,
    pub device_id: &'a str,
    pub id: Uuid,
    pub media_source_id: Option<Uuid>,
    pub width: u32,
    pub file: &'a str,
}

/// Resolves the set from cheap state, then serves its sheet from `root`.
pub(crate) async fn serve_sheet(
    ctx: &AppContext,
    root: &std::path::Path,
    req: SheetRequest<'_>,
) -> Response {
    if req.width != trickplay::WIDTH {
        return not_found();
    }
    let Some(n) = trickplay::parse_sheet(req.file) else {
        return not_found();
    };
    if !trickplay::enabled(&db::Settings::get_config_or_default(&ctx.db).await) {
        return not_found();
    }
    let (set_id, pictures) =
        match resolve_set(ctx, req.user_id, req.device_id, req.id, req.media_source_id)
            .await
        {
            Ok(found) => found,
            // No play and no PlaybackInfo pick for this device yet: a film or
            // an episode may still get pictures, and the Android TV (0.20.0)
            // and Android (2.7.3) clients keep a 404 through Coil and do not
            // ask again.
            Err(owner) => {
                return if can_have_pictures(ctx, owner).await {
                    not_ready()
                } else {
                    not_found()
                };
            }
        };
    let dir = trickplay::set_dir(root, set_id);
    // Any sheet on disk is whole: each is renamed into place once written, and
    // a pass that ends short by itself deletes the last one it wrote.
    let bytes = tokio::fs::read(trickplay::sheet_path(root, set_id, n))
        .await
        .ok()
        .filter(|b| !b.is_empty());
    let Some(bytes) = bytes else {
        let never = trickplay::is_done(&dir)
            || pictures.is_some_and(|p| u64::from(n) > trickplay::last_sheet(p));
        return if never { not_found() } else { not_ready() };
    };
    ctx.sessions
        .trickplay
        .mark_used(set_id, &dir);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "private, no-cache"),
        ],
        Body::from(bytes),
    )
        .into_response()
}

/// The set whose pictures this device gets for `id`: the one its play
/// registered, then the one of the stream its PlaybackInfo picked. Never a
/// stream lookup: that can refresh addon streams, players ask for a sheet on
/// nearly every move of the seek bar, and it could pick another file than
/// the one whose pictures exist. With the set, its picture count when known;
/// with no set, the item the request is for.
async fn resolve_set(
    ctx: &AppContext,
    user_id: Uuid,
    device_id: &str,
    id: Uuid,
    media_source_id: Option<Uuid>,
) -> Result<(Uuid, Option<u64>), Uuid> {
    let passes = &ctx
        .sessions
        .trickplay;
    if let Some(set_id) = passes.selected_set(id, device_id) {
        return Ok((set_id, passes.pictures_of(set_id)));
    }
    // A version held as the item id maps back to its owner, the key a play
    // registers under and PlaybackInfo saves its pick under.
    let (owner, source) =
        StreamService::version_target(ctx, Some(UserId(user_id)), id, media_source_id)
            .await;
    if let Some(set_id) = passes.selected_set(owner, device_id) {
        return Ok((set_id, passes.pictures_of(set_id)));
    }
    let Some(stream_id) =
        StreamService::selected_stream_id(ctx, Some(device_id), owner, source, None)
    else {
        return Err(owner);
    };
    Ok(trickplay::set_for_stream(ctx, stream_id).await)
}

/// Whether `owner` is a stored item that may still get pictures: a film, an
/// episode, or a stream or stream group not under a title or channel (see
/// [`trickplay::kind_has_pictures`], [`trickplay::manifest`] and
/// [`trickplay::set_id`]). One row read by primary key.
async fn can_have_pictures(ctx: &AppContext, owner: Uuid) -> bool {
    db::Media::get_by_id(&ctx.db, &owner)
        .await
        .ok()
        .flatten()
        .is_some_and(|item| trickplay::kind_has_pictures(&item.kind) && !item.is_live())
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-store")]).into_response()
}

/// A sheet that is not made yet. 503 and never kept, so an app asks again
/// when the seek bar comes back to it; the Android TV (0.20.0) and Android
/// (2.7.3) clients keep a 404 for good through Coil. No `Retry-After`: a
/// `Retry-After: 0` makes OkHttp, which those clients fetch through, ask
/// again at once.
fn not_ready() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::CACHE_CONTROL, "no-store")],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum_test::TestServer;
    use http::header::HeaderValue;
    use serde_json::json;
    use uuid::Uuid;

    use crate::{
        AppContext, Config, api, db,
        integration_test::{AUTH_HEADER, TestGuard, new_test_server_with_config},
        trickplay,
    };

    /// A test server whose data directory is `dir`, signed in as the test
    /// user, so sheets written for it never touch a real data directory.
    async fn server_in(dir: &std::path::Path) -> (TestServer, TestGuard, String) {
        let (server, guard) = new_test_server_with_config(Config {
            data_dir: dir.to_path_buf(),
            database_url: Some("sqlite::memory:".into()),
            torrent_http_port: None,
            torrent_peer_port: None,
            disable_dht: true,
            disable_startup_tasks: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let body: serde_json::Value = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER),
            )
            .json(&json!({ "Username": "test", "Pw": "test" }))
            .await
            .json();
        let token = body["AccessToken"]
            .as_str()
            .unwrap()
            .to_string();
        (server, guard, token)
    }

    async fn set_switch(ctx: &AppContext, on: bool) {
        let cfg = api::ServerConfiguration {
            trickplay_while_streaming: Some(on),
            ..db::Settings::get_config_or_default(&ctx.db).await
        };
        db::Settings::set_config(&ctx.db, &cfg)
            .await
            .unwrap();
    }

    /// A film with no stream rows, so nothing but the recorded selection can
    /// name a stream for it.
    async fn invented_film(ctx: &AppContext) -> db::Media {
        let external_ids = db::ExternalIds {
            imdb: db::NonEmptyString::try_new("tt9900001".to_string()).ok(),
            ..Default::default()
        };
        let mut film = db::Media {
            id: Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Movie,
                external_ids: external_ids.clone(),
                season: None,
                episode: None,
            }),
            title: "The Lantern Keeper".to_string(),
            kind: db::MediaKind::Movie,
            runtime: Some(7_200),
            external_ids,
            ..Default::default()
        };
        film.save(&ctx.db)
            .await
            .unwrap();
        film
    }

    /// Records that the test device's play of `item_id` reads `set_id`, as a
    /// play-start report does once its file is resolved.
    fn record_selection(ctx: &AppContext, item_id: Uuid, set_id: Uuid) {
        let passes = &ctx
            .sessions
            .trickplay;
        passes.register("invented-play", item_id, "test-device");
        assert!(passes.attach("invented-play", set_id));
    }

    fn write_sheet(ctx: &AppContext, set_id: Uuid, n: u32) {
        write_sheet_bytes(ctx, set_id, n, b"jpeg");
    }

    fn write_sheet_bytes(ctx: &AppContext, set_id: Uuid, n: u32, bytes: &[u8]) {
        let root = trickplay::root_dir(
            &ctx.config
                .data_dir,
        );
        std::fs::create_dir_all(trickplay::set_dir(&root, set_id)).unwrap();
        std::fs::write(trickplay::sheet_path(&root, set_id, n), bytes).unwrap();
    }

    /// A stored version of `film` measured by ffprobe at `secs` seconds.
    async fn version(
        ctx: &AppContext,
        film: &db::Media,
        idx: i64,
        secs: f64,
    ) -> db::Media {
        let name = format!("Lantern.Keeper.Version{idx}.mkv");
        let mut stream = db::Media {
            title: name.clone(),
            kind: db::MediaKind::Stream,
            parent_id: Some(film.id),
            idx: Some(idx),
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    name.clone()
                        .into(),
                ),
                filename: Some(name),
                ..Default::default()
            }),
            probe_data: Some(api::MediaSourceInfo {
                run_time_ticks: Some((secs * 10_000_000.0).round() as i64),
                remux: Some(api::MediaSourceRemuxInfo {
                    provider_info: None,
                    source: Some(api::ProbeOrigin::Ffprobe),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        stream
            .save(&ctx.db)
            .await
            .unwrap();
        stream
    }

    /// Saves `stream` as the version PlaybackInfo picked for `film` on
    /// `device_id`.
    fn pick(ctx: &AppContext, device_id: &str, film: &db::Media, stream: &db::Media) {
        crate::services::StreamService::save_device_selection(
            ctx, device_id, film.id, film.id, stream.id,
        );
    }

    /// Asks for sheet `file` of `film` as `device_id`, with no MediaSourceId.
    async fn ask(
        ctx: &AppContext,
        device_id: &str,
        film: &db::Media,
        file: &str,
    ) -> (http::StatusCode, Vec<u8>) {
        let (status, _, body) = ask_full(ctx, device_id, film, file).await;
        (status, body)
    }

    /// [`ask`], with the answer's headers.
    async fn ask_full(
        ctx: &AppContext,
        device_id: &str,
        film: &db::Media,
        file: &str,
    ) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
        let root = trickplay::root_dir(
            &ctx.config
                .data_dir,
        );
        let response = super::serve_sheet(
            ctx,
            &root,
            super::SheetRequest {
                user_id: Uuid::new_v4(),
                device_id,
                id: film.id,
                media_source_id: None,
                width: trickplay::WIDTH,
                file,
            },
        )
        .await;
        let status = response.status();
        let headers = response
            .headers()
            .clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, headers, body)
    }

    #[tokio::test]
    async fn serves_made_sheets_and_503s_those_not_made() {
        let dir = tempfile::tempdir().unwrap();
        let (server, guard, token) = server_in(dir.path()).await;
        let ctx = &guard.0;
        set_switch(ctx, true).await;
        let film = invented_film(ctx).await;
        let stream_id = Uuid::new_v4();
        record_selection(ctx, film.id, stream_id);
        write_sheet(ctx, stream_id, 0);
        let id = film
            .id
            .simple();

        let served = server
            .get(&format!("/Videos/{id}/Trickplay/320/0.jpg?ApiKey={token}"))
            .await;
        served.assert_status_ok();
        assert_eq!(served.header("content-type"), "image/jpeg");
        assert_eq!(
            served
                .as_bytes()
                .as_ref(),
            b"jpeg"
        );

        // Not made yet: 503, never kept, with no promise of when.
        let missing = server
            .get(&format!("/Videos/{id}/Trickplay/320/1.jpg?ApiKey={token}"))
            .expect_failure()
            .await;
        missing.assert_status(http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(missing.header("cache-control"), "no-store");
        let (status, headers, _) = ask_full(ctx, "test-device", &film, "1.jpg").await;
        assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            headers
                .get(http::header::RETRY_AFTER)
                .is_none()
        );

        // Missing from a finished set: it will never be made.
        let root = trickplay::root_dir(
            &ctx.config
                .data_dir,
        );
        std::fs::write(trickplay::set_dir(&root, stream_id).join("done"), b"").unwrap();
        let never = server
            .get(&format!("/Videos/{id}/Trickplay/320/1.jpg?ApiKey={token}"))
            .expect_failure()
            .await;
        never.assert_status_not_found();
        assert_eq!(never.header("cache-control"), "no-store");

        // Only one width is made.
        server
            .get(&format!("/Videos/{id}/Trickplay/480/0.jpg?ApiKey={token}"))
            .expect_failure()
            .await
            .assert_status_not_found();

        // No playlist of sheets.
        server
            .get(&format!(
                "/Videos/{id}/Trickplay/320/tiles.m3u8?ApiKey={token}"
            ))
            .expect_failure()
            .await
            .assert_status_not_found();

        set_switch(ctx, false).await;
        server
            .get(&format!("/Videos/{id}/Trickplay/320/0.jpg?ApiKey={token}"))
            .expect_failure()
            .await
            .assert_status_not_found();
    }

    #[tokio::test]
    async fn no_selection_is_503_without_a_stream_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let (server, guard, token) = server_in(dir.path()).await;
        let ctx = &guard.0;
        set_switch(ctx, true).await;
        let film = invented_film(ctx).await;
        // A fresh stored stream with a made sheet: a stream lookup would find
        // it, so only the route's refusal to look one up gives no 200. The
        // film may still get pictures once played: 503, not a 404 an app
        // keeps.
        let mut stream = db::Media {
            title: "Lantern.Keeper.1080p.mkv".to_string(),
            kind: db::MediaKind::Stream,
            parent_id: Some(film.id),
            idx: Some(0),
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    "Lantern.Keeper.1080p.mkv".into(),
                ),
                filename: Some("Lantern.Keeper.1080p.mkv".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        stream
            .save(&ctx.db)
            .await
            .unwrap();
        sqlx::query("UPDATE media SET streams_refreshed_at = ? WHERE id = ?")
            .bind(chrono::Utc::now().naive_utc())
            .bind(film.id)
            .execute(&ctx.db)
            .await
            .unwrap();
        write_sheet(ctx, stream.id, 0);
        let id = film
            .id
            .simple();

        let refused = server
            .get(&format!("/Videos/{id}/Trickplay/320/0.jpg?ApiKey={token}"))
            .expect_failure()
            .await;
        refused.assert_status(http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(refused.header("cache-control"), "no-store");
        let (status, headers, _) = ask_full(ctx, "test-device", &film, "0.jpg").await;
        assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            headers
                .get(http::header::RETRY_AFTER)
                .is_none()
        );

        // A stream not under a title keeps a set of its own and may still
        // get pictures: 503 too.
        let mut loose = db::Media {
            title: "Pattern.Loose.mkv".to_string(),
            kind: db::MediaKind::Stream,
            ..Default::default()
        };
        loose
            .save(&ctx.db)
            .await
            .unwrap();
        let loose_id = loose
            .id
            .simple();
        let waiting = server
            .get(&format!(
                "/Videos/{loose_id}/Trickplay/320/0.jpg?ApiKey={token}"
            ))
            .expect_failure()
            .await;
        waiting.assert_status(http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(waiting.header("cache-control"), "no-store");

        // An item that can never have pictures, and one not stored: 404.
        let mut channel = db::Media {
            title: "Pattern Channel".to_string(),
            kind: db::MediaKind::TvChannel,
            ..Default::default()
        };
        channel
            .save(&ctx.db)
            .await
            .unwrap();
        for item in [channel.id, Uuid::new_v4()] {
            let item = item.simple();
            let never = server
                .get(&format!(
                    "/Videos/{item}/Trickplay/320/0.jpg?ApiKey={token}"
                ))
                .expect_failure()
                .await;
            never.assert_status_not_found();
            assert_eq!(never.header("cache-control"), "no-store");
        }
    }

    #[tokio::test]
    async fn another_device_gets_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (_server, guard, _token) = server_in(dir.path()).await;
        let ctx = &guard.0;
        set_switch(ctx, true).await;
        let film = invented_film(ctx).await;
        let stream_id = Uuid::new_v4();
        record_selection(ctx, film.id, stream_id);
        write_sheet(ctx, stream_id, 0);
        let root = trickplay::root_dir(
            &ctx.config
                .data_dir,
        );

        let request = |device_id: &'static str| super::SheetRequest {
            user_id: Uuid::new_v4(),
            device_id,
            id: film.id,
            media_source_id: None,
            width: trickplay::WIDTH,
            file: "0.jpg",
        };
        let own = super::serve_sheet(ctx, &root, request("test-device")).await;
        assert_eq!(own.status(), http::StatusCode::OK);
        // Not this device's pictures: none for it yet, which it asks again
        // for once its own play has a set.
        let other = super::serve_sheet(ctx, &root, request("other-device")).await;
        assert_eq!(other.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn versions_of_one_count_share_a_set() {
        let dir = tempfile::tempdir().unwrap();
        let (_server, guard, _token) = server_in(dir.path()).await;
        let ctx = &guard.0;
        set_switch(ctx, true).await;
        let film = invented_film(ctx).await;
        let a = version(ctx, &film, 0, 9_391.756).await;
        let b = version(ctx, &film, 1, 9_391.776).await;
        let c = version(ctx, &film, 2, 7_800.0).await;
        assert_eq!(trickplay::set_id(&a), trickplay::set_id(&b));
        assert_ne!(trickplay::set_id(&a), trickplay::set_id(&c));
        write_sheet_bytes(ctx, trickplay::set_id(&a), 0, b"from version a");

        // A play of version B is served the pictures version A made.
        pick(ctx, "test-device", &film, &b);
        assert_eq!(
            ask(ctx, "test-device", &film, "0.jpg").await,
            (http::StatusCode::OK, b"from version a".to_vec())
        );
        // Version C has another length: nothing of A's set, only its own,
        // which is not made yet.
        pick(ctx, "test-device", &film, &c);
        let (status, _) = ask(ctx, "test-device", &film, "0.jpg").await;
        assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn a_sheet_past_the_count_is_404_and_one_within_it_503() {
        let dir = tempfile::tempdir().unwrap();
        let (_server, guard, _token) = server_in(dir.path()).await;
        let ctx = &guard.0;
        set_switch(ctx, true).await;
        let film = invented_film(ctx).await;
        // 9,391.756 s: 940 pictures, sheets 0 to 37.
        let v = version(ctx, &film, 0, 9_391.756).await;
        write_sheet(ctx, trickplay::set_id(&v), 0);
        pick(ctx, "test-device", &film, &v);
        let (status, _) = ask(ctx, "test-device", &film, "37.jpg").await;
        assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
        let (status, _) = ask(ctx, "test-device", &film, "38.jpg").await;
        assert_eq!(status, http::StatusCode::NOT_FOUND);

        // A play's recorded set whose count no pass has given yet: not known,
        // so 503.
        let unknown = Uuid::new_v4();
        record_selection(ctx, film.id, unknown);
        let (status, _) = ask(ctx, "test-device", &film, "38.jpg").await;
        assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn a_stopped_plays_set_is_not_served_to_the_next_version() {
        let dir = tempfile::tempdir().unwrap();
        let (_server, guard, _token) = server_in(dir.path()).await;
        let ctx = &guard.0;
        set_switch(ctx, true).await;
        let film = invented_film(ctx).await;
        let long = version(ctx, &film, 0, 9_391.756).await;
        let short = version(ctx, &film, 1, 7_800.0).await;
        write_sheet_bytes(ctx, trickplay::set_id(&long), 0, b"long cut");
        write_sheet_bytes(ctx, trickplay::set_id(&short), 0, b"short cut");

        record_selection(ctx, film.id, trickplay::set_id(&long));
        pick(ctx, "test-device", &film, &short);
        assert_eq!(
            ask(ctx, "test-device", &film, "0.jpg").await,
            (http::StatusCode::OK, b"long cut".to_vec())
        );
        // The long play stops; PlaybackInfo has picked the short version and
        // no start report has come yet: the short cut's own set, at once.
        ctx.sessions
            .trickplay
            .release("invented-play");
        assert_eq!(
            ask(ctx, "test-device", &film, "0.jpg").await,
            (http::StatusCode::OK, b"short cut".to_vec())
        );
    }

    #[tokio::test]
    async fn a_sheet_is_served_whatever_is_missing_below_it() {
        let dir = tempfile::tempdir().unwrap();
        let (_server, guard, _token) = server_in(dir.path()).await;
        let ctx = &guard.0;
        set_switch(ctx, true).await;
        let film = invented_film(ctx).await;
        let holed = version(ctx, &film, 0, 9_391.756).await;
        let holed_set = trickplay::set_id(&holed);
        write_sheet(ctx, holed_set, 3);
        write_sheet(ctx, holed_set, 4);

        // The device with a play gets what exists at once, with sheets 0 to
        // 2 missing and the set not finished.
        record_selection(ctx, film.id, holed_set);
        let (status, _) = ask(ctx, "test-device", &film, "3.jpg").await;
        assert_eq!(status, http::StatusCode::OK);
        // So does a device with no play, whose PlaybackInfo picked this
        // version.
        pick(ctx, "other-device", &film, &holed);
        let (status, _) = ask(ctx, "other-device", &film, "3.jpg").await;
        assert_eq!(status, http::StatusCode::OK);
        // A sheet in the hole is not made yet.
        let (status, _) = ask(ctx, "test-device", &film, "0.jpg").await;
        assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);

        // An empty sheet is never served; it is made again.
        write_sheet_bytes(ctx, holed_set, 5, b"");
        let (status, _) = ask(ctx, "test-device", &film, "5.jpg").await;
        assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);

        // A sheet missing from a finished set is never made.
        let root = trickplay::root_dir(
            &ctx.config
                .data_dir,
        );
        std::fs::write(trickplay::set_dir(&root, holed_set).join("done"), b"").unwrap();
        let (status, _) = ask(ctx, "other-device", &film, "3.jpg").await;
        assert_eq!(status, http::StatusCode::OK);
        let (status, _) = ask(ctx, "other-device", &film, "1.jpg").await;
        assert_eq!(status, http::StatusCode::NOT_FOUND);
    }
}
