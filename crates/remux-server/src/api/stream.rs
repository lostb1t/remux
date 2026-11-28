use axum::{
    extract::{Path, State},
    response::IntoResponse,
};
use axum_extra::extract::Query;
use remux_macros::{get, query};
use uuid::Uuid;

use crate::{OptionExt, ResultExt};
use axum_anyhow::ApiResult as Result;

use crate::{AppState, db, stream::StreamDescriptor};

#[query]
pub struct StreamLinkQuery {
    pub token: Option<String>,
}

/// Proxy any stream stored in `db::Media.stream_info` to the caller.
///
/// Handles all URL schemes transparently via [`crate::stream::StreamSource`].
/// Addon-owned streams (`Opendal`) are dispatched to the addon's `serve_stream`.
///
/// Only for this server's own ffmpeg and ffprobe, which send no credentials:
/// the link must carry the token [`StreamDescriptor::server_input`] puts on it.
/// Stream ids are derived from public data (the title's id and the torrent
/// hash), so without the token anyone could stream through this server, past
/// every user policy.
#[get("/stream/{id}")]
pub async fn stream_proxy(
    headers: axum::http::HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<StreamLinkQuery>,
) -> Result<impl IntoResponse> {
    if !q
        .token
        .as_deref()
        .is_some_and(|token| crate::stream::is_stream_link_token(id, token))
    {
        return Err(anyhow::anyhow!("missing or invalid stream link token"))
            .context_not_found("not found");
    }
    let mut media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?;
    // The listing is taken out of the row; the rest is kept for the tap.
    let descriptor = match media.as_mut() {
        Some(media) => media
            .stream_info
            .take()
            .map(|si| si.descriptor)
            .context_not_found("media has no URL")?,
        // A stream an addon kept for seek previews only gave: never a media
        // row, so this route, with the token, is the only way it is read.
        None => {
            db::PreviewSource::get_fresh(
                &state
                    .ctx
                    .db,
                id,
                chrono::Utc::now().naive_utc(),
            )
            .await?
            .context_not_found("not found")?
            .stream_info
            .descriptor
        }
    };

    let response = serve(&state, descriptor, &headers).await?;
    // A preview source has no row, and its length is measured apart.
    Ok(match &media {
        Some(media) => crate::stream::tap_length(&state.ctx, media, response).await,
        None => response,
    })
}

async fn serve(
    state: &AppState,
    descriptor: StreamDescriptor,
    headers: &axum::http::HeaderMap,
) -> Result<axum::response::Response> {
    if let Some(addon_id) = descriptor.addon_id() {
        let addon = state
            .ctx
            .addons
            .get(addon_id)
            .context_not_found("addon not found")?;
        let stream = addon
            .stream
            .as_ref()
            .context_not_found("addon does not support streams")?;
        return stream
            .serve_stream(&descriptor, headers)
            .await;
    }

    // Live streams go through the transcoder, never this proxy.
    if matches!(descriptor, StreamDescriptor::Rtsp { .. }) {
        return Err(anyhow::anyhow!("rtsp streams are not proxied"))
            .context_bad_request("RTSP streams play through the transcoder");
    }

    if matches!(descriptor, StreamDescriptor::Torrent { .. }) {
        let cfg = db::Settings::get_config_or_default(
            &state
                .ctx
                .db,
        )
        .await;
        if !cfg
            .p2p_enabled
            .unwrap_or(true)
        {
            return Err(anyhow::anyhow!("P2P disabled")).context_bad_request(
                "P2P streams are disabled by the server administrator",
            );
        }
    }

    descriptor
        .into_source()
        .serve(state, headers)
        .await
}

#[cfg(test)]
mod tests {
    use crate::{
        integration_test::{authenticated_server, insert_test_source},
        stream::{StreamDescriptor, stream_link_token},
    };

    /// The proxy serves only links this server made for its own ffmpeg: a
    /// stream id alone, which anyone can derive, gets nothing.
    #[tokio::test]
    async fn stream_proxy_requires_the_servers_own_link_token() {
        let (server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;
        let dir = tempfile::tempdir().unwrap();
        let file = dir
            .path()
            .join("movie.mkv");
        std::fs::write(&file, b"release").unwrap();
        let mut source = insert_test_source(ctx).await;
        source
            .stream_info
            .as_mut()
            .unwrap()
            .descriptor = StreamDescriptor::Local(file);
        source
            .save(&ctx.db)
            .await
            .unwrap();
        let id = source.id;

        server
            .get(&format!("/stream/{id}"))
            .expect_failure()
            .await
            .assert_status_not_found();
        server
            .get(&format!(
                "/stream/{id}?token={}",
                stream_link_token(uuid::Uuid::new_v4())
            ))
            .expect_failure()
            .await
            .assert_status_not_found();

        let served = server
            .get(&format!("/stream/{id}?token={}", stream_link_token(id)))
            .await;
        served.assert_status_ok();
        assert_eq!(
            served
                .as_bytes()
                .as_ref(),
            b"release"
        );
    }

    /// A stream an addon kept for seek previews only gave is served to the
    /// server's own reader by its id and token, and is nowhere among the
    /// title's versions.
    #[tokio::test]
    async fn stream_proxy_serves_a_preview_source_only_with_the_token() {
        let (server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;
        let dir = tempfile::tempdir().unwrap();
        let file = dir
            .path()
            .join("small.mkv");
        std::fs::write(&file, b"small release").unwrap();
        let mut film = crate::db::Media {
            title: "Invented Film".to_string(),
            kind: crate::db::MediaKind::Movie,
            external_ids: crate::db::ExternalIds {
                tmdb: Some(1),
                ..Default::default()
            },
            ..Default::default()
        };
        film.save(&ctx.db)
            .await
            .unwrap();
        let addon = uuid::Uuid::new_v4();
        crate::db::PreviewSource::store(
            &ctx.db,
            film.id,
            addon,
            &[crate::db::NewPreviewSource {
                content_key: "local:small".to_string(),
                stream_info: crate::stream::StreamInfo {
                    descriptor: StreamDescriptor::Local(file),
                    addon_id: Some(addon),
                    ..Default::default()
                },
                probe_data: None,
            }],
            chrono::Utc::now().naive_utc(),
        )
        .await
        .unwrap();
        let id = crate::db::PreviewSource::id_for(film.id, addon, "local:small");

        assert!(
            crate::db::Media::get_by_id(&ctx.db, &id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            film.streams(&ctx.db)
                .await
                .unwrap()
                .is_empty()
        );
        server
            .get(&format!("/stream/{id}"))
            .expect_failure()
            .await
            .assert_status_not_found();
        let served = server
            .get(&format!("/stream/{id}?token={}", stream_link_token(id)))
            .await;
        served.assert_status_ok();
        assert_eq!(
            served
                .as_bytes()
                .as_ref(),
            b"small release"
        );
    }

    // The length a relayed file states, read on the way.

    /// A made-up Matroska file stating `seconds`, followed by made-up media
    /// bytes.
    fn matroska(seconds: f64) -> Vec<u8> {
        let mut info = vec![0x2A, 0xD7, 0xB1, 0x83, 0x0F, 0x42, 0x40];
        info.extend([0x44, 0x89, 0x88]);
        info.extend((seconds * 1_000.0).to_be_bytes());
        let mut out = vec![0x1A, 0x45, 0xDF, 0xA3, 0x84, 0x42, 0x82, 0x81, b'w'];
        out.extend([0x18, 0x53, 0x80, 0x67, 0xFF]);
        out.extend([0x15, 0x49, 0xA9, 0x66, 0x80 | info.len() as u8]);
        out.extend(info);
        out.extend([0x1F, 0x43, 0xB6, 0x75, 0xFF]);
        out.extend(vec![0xA5; 200_000]);
        out
    }

    fn mp4_box(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = ((data.len() + 8) as u32)
            .to_be_bytes()
            .to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out
    }

    /// A made-up MP4 file stating `seconds`, its index at the end, and where
    /// that index starts.
    fn mp4_index_last(seconds: u32) -> (Vec<u8>, usize) {
        let mut out = mp4_box(b"ftyp", b"isom\0\0\0\0isom");
        out.extend(mp4_box(b"mdat", &vec![0x5A; 100_000]));
        let at = out.len();
        let mut mvhd = vec![0u8; 12];
        mvhd.extend(1_000u32.to_be_bytes());
        mvhd.extend((seconds * 1_000).to_be_bytes());
        mvhd.extend([0u8; 80]);
        out.extend(mp4_box(b"moov", &mp4_box(b"mvhd", &mvhd)));
        (out, at)
    }

    const LISTED_TICKS: i64 = 6_180 * 10_000_000;

    /// A made-up TMDB id for each film, which must differ.
    static NEXT_TMDB: std::sync::atomic::AtomicI64 =
        std::sync::atomic::AtomicI64::new(100);

    /// A version of a made-up film stored with the film's listed runtime
    /// and a file name guess, served from `bytes` on disk; seek previews on
    /// or off.
    async fn unmeasured_version(
        ctx: &crate::AppContext,
        dir: &std::path::Path,
        bytes: &[u8],
        on: bool,
    ) -> crate::db::Media {
        crate::db::Settings::set_config(
            &ctx.db,
            &crate::api::ServerConfiguration {
                trickplay_while_streaming: Some(on),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        if on {
            // An addon that may be read for previews, as there is wherever
            // they are made: without one, the relay is not looked at.
            let now = chrono::Utc::now().naive_utc();
            ctx.sessions
                .trickplay
                .set_addon_rules(&[crate::addons::Addon {
                    id: uuid::Uuid::new_v4(),
                    name: "Invented Addon".to_string(),
                    preset: crate::addons::AddonPresetRef {
                        kind: "stremio".to_string(),
                        ..Default::default()
                    },
                    resources: vec![
                        remux_sdks::stremio::ResourceType::Stream,
                        remux_sdks::stremio::ResourceType::SeekPreviews,
                    ],
                    types: None,
                    enabled: true,
                    priority: 0,
                    created_at: now,
                    updated_at: now,
                    system: false,
                    is_default: true,
                    http_redirect_stream: false,
                    subtitle_extraction: false,
                    probe_on_scan: false,
                    service_filter: vec![],
                }]);
        }
        let file = dir.join("version.bin");
        std::fs::write(&file, bytes).unwrap();
        let mut film = crate::db::Media {
            title: "Invented Film".to_string(),
            kind: crate::db::MediaKind::Movie,
            external_ids: crate::db::ExternalIds {
                tmdb: Some(NEXT_TMDB.fetch_add(1, std::sync::atomic::Ordering::SeqCst)),
                ..Default::default()
            },
            ..Default::default()
        };
        film.save(&ctx.db)
            .await
            .unwrap();
        let mut version = crate::db::Media {
            title: "Invented.Film.1080p.mkv".to_string(),
            kind: crate::db::MediaKind::Stream,
            parent_id: Some(film.id),
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: StreamDescriptor::Local(file),
                ..Default::default()
            }),
            probe_data: Some(crate::api::MediaSourceInfo {
                run_time_ticks: Some(LISTED_TICKS),
                media_streams: vec![crate::api::MediaStream {
                    type_: crate::api::MediaStreamType::Video,
                    codec: Some("h264".to_string()),
                    ..Default::default()
                }],
                remux: Some(crate::api::MediaSourceRemuxInfo {
                    provider_info: None,
                    source: Some(crate::api::ProbeOrigin::FilenameGuess),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        version
            .save(&ctx.db)
            .await
            .unwrap();
        version
    }

    async fn probe_of(
        ctx: &crate::AppContext,
        id: uuid::Uuid,
    ) -> crate::api::MediaSourceInfo {
        crate::db::Media::get_by_id(&ctx.db, &id)
            .await
            .unwrap()
            .unwrap()
            .probe_data
            .unwrap()
    }

    /// What a relay may change: the length and its origin, and the tracks it
    /// must keep.
    fn facts(
        probe: &crate::api::MediaSourceInfo,
    ) -> (
        Option<i64>,
        Option<crate::api::ProbeOrigin>,
        Vec<crate::api::MediaStream>,
    ) {
        (
            probe.run_time_ticks,
            probe
                .remux
                .as_ref()
                .and_then(|r| r.source),
            probe
                .media_streams
                .clone(),
        )
    }

    /// The measured length the row has, once it has one, waiting a little
    /// for the background save.
    async fn length_stored(ctx: &crate::AppContext, id: uuid::Uuid) -> Option<i64> {
        for _ in 0..100 {
            let row = crate::db::Media::get_by_id(&ctx.db, &id)
                .await
                .unwrap()
                .unwrap();
            if let Some(ticks) = crate::trickplay::measured_ticks(&row) {
                return Some(ticks);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        None
    }

    /// Long enough for a background save that was going to happen.
    async fn settle() {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }

    async fn get_range(
        server: &axum_test::TestServer,
        id: uuid::Uuid,
        range: Option<&str>,
    ) -> axum_test::TestResponse {
        let request =
            server.get(&format!("/stream/{id}?token={}", stream_link_token(id)));
        match range {
            Some(range) => {
                request
                    .add_header(
                        axum::http::header::RANGE,
                        axum::http::HeaderValue::from_str(range).unwrap(),
                    )
                    .await
            }
            None => request.await,
        }
    }

    /// A relayed file's stated length is stored on its row, as a measured
    /// one, with every other fact kept, and the bytes arrive unchanged.
    #[tokio::test]
    async fn a_relayed_matroska_file_gets_its_stated_length() {
        let (server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;
        let dir = tempfile::tempdir().unwrap();
        let bytes = matroska(6_206.528);
        let version = unmeasured_version(ctx, dir.path(), &bytes, true).await;
        assert!(crate::trickplay::measured_ticks(&version).is_none());

        let served = get_range(&server, version.id, None).await;
        served.assert_status_ok();
        assert_eq!(
            served
                .as_bytes()
                .as_ref(),
            &bytes[..]
        );
        assert_eq!(length_stored(ctx, version.id).await, Some(62_065_280_000));
        let probe = probe_of(ctx, version.id).await;
        assert_eq!(
            probe
                .remux
                .as_ref()
                .and_then(|r| r.source),
            Some(crate::api::ProbeOrigin::Container)
        );
        assert_eq!(
            probe.media_streams,
            version
                .probe_data
                .as_ref()
                .unwrap()
                .media_streams
        );
        assert!(probe.is_filename_guess(), "its tracks are still a guess");

        // Measured now: the next relay is not read, and changes nothing.
        let served = get_range(&server, version.id, Some("bytes=0-")).await;
        served.assert_status(axum::http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            served
                .as_bytes()
                .as_ref(),
            &bytes[..]
        );
        settle().await;
        assert_eq!(facts(&probe_of(ctx, version.id).await), facts(&probe));

        // A filename guess made from the row as it was read before is not
        // saved over the length.
        let stale = crate::conversions::filename_guess_persist_payload(
            version
                .probe_data
                .as_ref()
                .unwrap(),
        );
        crate::conversions::save_filename_guess(&ctx.db, &version.id, stale)
            .await
            .unwrap();
        assert_eq!(facts(&probe_of(ctx, version.id).await), facts(&probe));
    }

    #[tokio::test]
    async fn a_measured_row_and_a_server_with_seek_previews_off_are_not_read() {
        let (server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;
        let dir = tempfile::tempdir().unwrap();
        let bytes = matroska(6_206.528);

        let off = unmeasured_version(ctx, dir.path(), &bytes, false).await;
        let served = get_range(&server, off.id, None).await;
        served.assert_status_ok();
        assert_eq!(
            served
                .as_bytes()
                .as_ref(),
            &bytes[..]
        );
        settle().await;
        assert_eq!(
            facts(&probe_of(ctx, off.id).await),
            facts(
                off.probe_data
                    .as_ref()
                    .unwrap()
            )
        );

        let measured = unmeasured_version(ctx, dir.path(), &bytes, true).await;
        let probed = crate::api::MediaSourceInfo {
            run_time_ticks: Some(LISTED_TICKS),
            remux: Some(crate::api::MediaSourceRemuxInfo {
                provider_info: None,
                source: Some(crate::api::ProbeOrigin::Ffprobe),
            }),
            ..Default::default()
        };
        crate::db::Media::save_probe_data(&ctx.db, &measured.id, &probed)
            .await
            .unwrap();
        let served = get_range(&server, measured.id, None).await;
        served.assert_status_ok();
        settle().await;
        assert_eq!(facts(&probe_of(ctx, measured.id).await), facts(&probed));
    }

    #[tokio::test]
    async fn only_a_range_from_the_start_or_an_mp4_index_is_read() {
        let (server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;
        let dir = tempfile::tempdir().unwrap();

        // A Matroska range from elsewhere.
        let bytes = matroska(6_206.528);
        let version = unmeasured_version(ctx, dir.path(), &bytes, true).await;
        let served = get_range(&server, version.id, Some("bytes=100-")).await;
        served.assert_status(axum::http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            served
                .as_bytes()
                .as_ref(),
            &bytes[100..]
        );
        settle().await;
        assert_eq!(
            facts(&probe_of(ctx, version.id).await),
            facts(
                version
                    .probe_data
                    .as_ref()
                    .unwrap()
            )
        );

        // An MP4 whose index is at the end: nothing from the start, then the
        // client's range for the index.
        let dir = tempfile::tempdir().unwrap();
        let (bytes, at) = mp4_index_last(6_207);
        let version = unmeasured_version(ctx, dir.path(), &bytes, true).await;
        get_range(&server, version.id, Some("bytes=0-65535"))
            .await
            .assert_status(axum::http::StatusCode::PARTIAL_CONTENT);
        settle().await;
        assert_eq!(
            facts(&probe_of(ctx, version.id).await),
            facts(
                version
                    .probe_data
                    .as_ref()
                    .unwrap()
            )
        );
        let served =
            get_range(&server, version.id, Some(format!("bytes={at}-").as_str())).await;
        served.assert_status(axum::http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            served
                .as_bytes()
                .as_ref(),
            &bytes[at..]
        );
        assert_eq!(length_stored(ctx, version.id).await, Some(62_070_000_000));
    }

    /// A response relaying `pieces`, as a source that sends them one by one.
    fn relay_of(pieces: Vec<std::io::Result<Vec<u8>>>) -> axum::response::Response {
        let pieces = futures_util::stream::iter(
            pieces
                .into_iter()
                .map(|piece| piece.map(axum::body::Bytes::from)),
        );
        axum::response::Response::builder()
            .status(axum::http::StatusCode::OK)
            .body(axum::body::Body::from_stream(pieces))
            .unwrap()
    }

    /// The made-up file in three pieces: its header ends in the second.
    fn three_pieces(bytes: &[u8]) -> Vec<std::io::Result<Vec<u8>>> {
        vec![
            Ok(bytes[..20].to_vec()),
            Ok(bytes[20..64].to_vec()),
            Ok(bytes[64..].to_vec()),
        ]
    }

    /// A relay dropped part way through the header, as when the client goes
    /// away, stores nothing; the same body read whole does.
    #[tokio::test]
    async fn a_relay_dropped_inside_the_header_stores_nothing() {
        use futures_util::StreamExt;
        let (_server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;
        let dir = tempfile::tempdir().unwrap();
        let bytes = matroska(6_206.528);
        let version = unmeasured_version(ctx, dir.path(), &bytes, true).await;

        let relayed =
            crate::stream::tap_length(ctx, &version, relay_of(three_pieces(&bytes)))
                .await;
        let mut body = relayed
            .into_body()
            .into_data_stream();
        assert_eq!(
            body.next()
                .await
                .unwrap()
                .unwrap()
                .as_ref(),
            &bytes[..20]
        );
        drop(body);
        settle().await;
        assert_eq!(
            facts(&probe_of(ctx, version.id).await),
            facts(
                version
                    .probe_data
                    .as_ref()
                    .unwrap()
            )
        );

        let relayed =
            crate::stream::tap_length(ctx, &version, relay_of(three_pieces(&bytes)))
                .await;
        let mut body = relayed
            .into_body()
            .into_data_stream();
        let mut got = Vec::new();
        while let Some(piece) = body
            .next()
            .await
        {
            got.extend_from_slice(&piece.unwrap());
        }
        assert_eq!(got, bytes);
        assert_eq!(length_stored(ctx, version.id).await, Some(62_065_280_000));
    }

    /// A relay that breaks off after the header has passed, as when the
    /// source goes away, stores the length and passes the error on.
    #[tokio::test]
    async fn a_relay_broken_off_after_the_header_stores_its_length() {
        use futures_util::StreamExt;
        let (_server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;
        let dir = tempfile::tempdir().unwrap();
        let bytes = matroska(6_206.528);
        let version = unmeasured_version(ctx, dir.path(), &bytes, true).await;
        let pieces = vec![
            Ok(bytes[..64].to_vec()),
            Err(std::io::Error::other("connection closed")),
        ];
        let relayed = crate::stream::tap_length(ctx, &version, relay_of(pieces)).await;
        let mut body = relayed
            .into_body()
            .into_data_stream();
        assert_eq!(
            body.next()
                .await
                .unwrap()
                .unwrap()
                .as_ref(),
            &bytes[..64]
        );
        assert!(
            body.next()
                .await
                .unwrap()
                .is_err()
        );
        drop(body);
        assert_eq!(length_stored(ctx, version.id).await, Some(62_065_280_000));
        let probe = probe_of(ctx, version.id).await;
        assert_eq!(
            probe
                .remux
                .as_ref()
                .and_then(|r| r.source),
            Some(crate::api::ProbeOrigin::Container)
        );
    }

    #[test]
    fn a_stream_link_token_only_fits_its_own_id() {
        let id = uuid::Uuid::new_v4();
        let token = stream_link_token(id);
        assert!(crate::stream::is_stream_link_token(id, &token));
        assert!(!crate::stream::is_stream_link_token(
            uuid::Uuid::new_v4(),
            &token
        ));
        assert!(!crate::stream::is_stream_link_token(id, ""));
        assert!(!crate::stream::is_stream_link_token(id, "zz"));
        assert!(!crate::stream::is_stream_link_token(
            id,
            &token[..token.len() - 1]
        ));
    }
}
