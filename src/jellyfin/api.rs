use crate::errors::LogErr;
use crate::jellyfin::MediaSourceInfo;
use crate::jellyfin::MediaStream;
use crate::sdks::CachedEndpoint;
use anyhow::Context;
use anyhow::anyhow;
use axum::Extension;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::response::Redirect;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::routing::get_service;
use axum::routing::post;
use axum_extra::extract::Query;
use axum_extra::response::file_stream::FileStream;
use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use chrono;
use ffprobe;
use futures::future::join_all;
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use futures_util::stream::Stream;
use headers;
use http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::io;
use std::str::FromStr;
use std::time::Duration;
use tokio_util::io::ReaderStream;
use tokio_util::io::StreamReader;
use tower_http::services::{ServeDir, ServeFile};
use tracing::info;
use tracing::trace;
use tracing::warn;
use uuid::Uuid;

//use crate::db;
//use anyhow::Result;
use crate::AppState;
use crate::conversions;
use crate::conversions::stream_into_media_source_info;
use crate::db;
use crate::db::auth;
use crate::db::auth::Device;
use crate::db::user::User;
use crate::jellyfin;
use crate::rewrite_request_uri;
use crate::sdks;
use crate::sdks::{aio, tmdb};
use crate::utils;
use crate::utils::IntoVec;
use crate::utils::server_id;
use axum_anyhow::{ApiResult as Result, OptionExt, ResultExt};
use chrono::Datelike;
use tower::util::MapRequestLayer;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users/authenticatebyname", post(users_authenticatebyname))
        .route("/system/info/public", get(system_info_public))
        .route("/users/public", get(system_info_public))
        .route("/system/ping", get(system_ping))
        .route("/system/endpoint", get(system_endpoint))
        .route("/userviews", get(userviews))
        .route("/userviews/groupingoptions", get(userviews_groupingoptions))
        .route("/library/virtualfolders", get(library_virtualfolders))
        .route("/items/suggestions", get(items_suggestions))
        .route("/shows/{id}/seasons", get(shows_seasons))
        .route("/shows/{id}/episodes", get(shows_episodes))
        .route("/items/latest", get(items_flat))
        .route("/persons", get(persons))
        .route("/items", get(items))
        .route("/Items", get(items))
        .route("/items/{id}/images/{image_type}", get(items_images))
        .route("/items/{id}/images/{image_type}/{index}", get(items_images))
        .route("/items/{id}", get(items_get))
        .route("/items/{id}/playbackinfo", post(items_playbackinfo))
        .route("/items/filters", get(items_filters))
        .route("/users/me", get(users_me))
        .route("/users/{user_id}", get(users_me))
        .route("/users/{user_id}/items", get(items))
        .route("/users/{user_id}/items/latest", get(items_flat))
        .route("/users/{user_id}/items/{id}", get(users_items_get))
        .route("/users/{user_id}/views", get(userviews))
        .route(
            "/users/{user_id}/groupingoptions",
            get(users_groupingoptions),
        )
        .route("/videos/{id}/stream", get(videos_stream))
        .route("/playback/bitratetest", get(playback_bitratetest))
        .route("/displaypreferences/usersettings", get(user_settings))
        .route("/system/info", get(system_info))
        // .route("/videos/master.m3u8", get(master_hls))
        // stubs. to implement
        .route("/shows/nextup", get(mock_items))
        .route("/users/{user_id}/items/resume", get(mock_items))
        .route("/users/{user_id}/items/similar", get(mock_items))
        .route("/users/{user_id}/intros", get(mock_items))
        .route("/users/{user_id}/items/{id}/intros", get(mock_items))
        .route("/users/{user_id}/configuration", post(stub))
        .route("/items/{id}/similar", get(mock_items))
        .route("/items/{id}/thememedia", get(stub_json))
        .route("/useritems/resume", get(mock_items))
        .route("/sessions/playing", post(stub))
        .route("/sessions/playing/progress", post(stub))
        .route("/sessions/playing/stopped", post(stub))
        .route("/userimage", get(user_image))
        .route("/sessions/capabilities/full", post(stub))
        .route("/quickconnect/enabled", post(stub))
        .route("/branding/configuration", post(stub))
        .route("/branding/configuration", get(stub))
        .route("/quickconnect/enabled", get(stub))
        .route("/syncplay/list", get(mock_items))

    //.map_request(rewrite_request_uri)
    //.layer(MapRequestLayer::new(rewrite_request_uri))
    // .route("/jellyfin/Items/{id}/Image/{image_type}", get_service(ServeFile::new("assets/placeholder_poster.jpg")))
}

//#[route(get, "/jellyfin/System/Info/Public")]

/// TODO: make a real server id
pub async fn system_info_public(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::PublicSystemInfo {
        local_address: Some("".to_string()),
        server_name: Some("Remux".to_string()),
        product_name: Some("Jellyfin Server".to_string()),
        startup_wizard_completed: Some(true),
        version: Some("10.10.7".to_string()),
        operating_system: Some("".to_string()),
        id: Some(server_id()),
        ..Default::default()
    }))
}

pub async fn system_info(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::SystemInfo {
        id: Some(server_id()),
        server_name: Some(server_id()),
        // server_id: Some(server_id()),
        ..Default::default()
    }))
}

pub async fn system_ping(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(json!("Remux Server")))
}

pub async fn system_endpoint(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json(json!({
        "IsLocal": false,
        "IsInNetwork": false,

    })))
}

pub async fn user_configuration_update(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(payload): Json<jellyfin::UserConfiguration>,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::UserConfiguration::default()))
}

pub async fn user_settings(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::DisplayPreferencesDto {
        id: Some("test".to_string()),
        ..Default::default()
    }))
}

pub async fn users_authenticatebyname(
    State(state): State<AppState>,
    auth_header: auth::JellyfinAuthHeader,
    Json(data): Json<jellyfin::AuthenticateUserByName>,
) -> Result<impl IntoResponse> {
    let user = User::authenticate(&state.ctx.db, &data.username, &data.pw)
        .await?
        .context_unauthorized("not found", "not foubd")?;
    let device = Device::new_from_header(auth_header, &user)?;
    device.save(&state.ctx.db).await?;

    Ok(Json(jellyfin::AuthenticationResult {
        access_token: Some(device.access_token),
        server_id: server_id(),
        user: Some(user.into()),
        ..Default::default()
    }))
}

/// This sbould hold dynamic collections
pub async fn userviews(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let manifest = session.aio.get_manifest().await?;

    //let mut items = vec![jellyfin::BaseItemDto {
    //     name: Some("Collections".to_string()),
    //    id: state.config.collection_id.clone(),
    //    collection_type: Some(jellyfin::CollectionType::Boxsets),
    //    is_folder: true,
    //    ..Default::default()
    //}];

    let items = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Catalog]),
            promoted: Some(true),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .map(|x| {
        let mut item: jellyfin::BaseItemDto = x.into();
        item.type_ = jellyfin::MediaType::CollectionFolder;
        item.collection_type = Some(jellyfin::CollectionType::Movies);
        item
    })
    .collect::<Vec<jellyfin::BaseItemDto>>();

    // items.extend(libs);

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items,
        ..Default::default()
    }))
}

pub async fn userviews_groupingoptions(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let manifest = session.aio.get_manifest().await?;

    // Ok(Json(json!(
    // )))
    Ok(StatusCode::NO_CONTENT.into_response())
    // Ok(Json(json!(
    //     crate::jellyfin::get_virtual_folders(&state).await?
    // )))
}

pub async fn library_virtualfolders(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let manifest = session.aio.get_manifest().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
    // Ok(Json(json!(
    //     crate::jellyfin::get_virtual_folders(&state).await?
    // )))
}

pub async fn items_suggestions(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    //let b = state.tmdb.movie_popular_list().send().await.unwrap()
    //.into_inner()
    //.results
    //.map(|c| {
    //  jellyfin::BaseItemDto {
    //     name: c.title,
    //     ..Default::default()
    //   }
    //}
    //);
    //let tmdb_items = state.tmdb.movie_now_playing().send().await;
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: vec![],
        ..Default::default()
    }))
}

pub async fn persons(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: vec![],
        ..Default::default()
    }))
}

pub struct ItemsQueryResult {
    pub items: Vec<jellyfin::BaseItemDto>,
    pub total_count: i64,
}

pub async fn get_items(
    state: AppState,
    session: auth::AuthSession,
    mut q: jellyfin::GetItemsQuery,
    _count: bool,
) -> Result<ItemsQueryResult> {
    // trace!(?q, "get_items");
    let aio = session.aio;

    let parent = if let Some(parent_id) = q.parent_id.clone() {
        db::Media::get_by_id(&state.ctx.db, &parent_id).await?
    } else {
        None
    };
    //let aio_search = session.user.get_aio_search()?;
    let search = q.search_term.clone().or(q.name_starts_with.clone());
    let skip = q.start_index.unwrap_or(0) as u32;

    //  trace!(?q, "get_items");

    // only support Movie and Series for search and catalogs
    if search.is_some()
        || parent
            .clone()
            .map_or(false, |p| p.kind == db::MediaKind::Catalog)
    {
        let types = q.get_requested_item_types();
        // if types.len() != 0 {
        if types.len() == 0
            || ![jellyfin::MediaType::Movie, jellyfin::MediaType::Series]
                .contains(&types[0])
        {
            return Ok(ItemsQueryResult {
                items: vec![],
                total_count: 0,
            });
        }

        if let Some(s) = search {
            // todo: need to to make parallel request for types

            let items = aio
                .search(types[0].clone().into(), s)
                .await?
                .into_iter()
                .filter_map(|meta| match db::Media::try_from(meta.clone()) {
                    Ok(media) => {
                        state.ctx.store.save(
                            media.id.clone(),
                            meta.clone(),
                            Duration::from_secs(360),
                        );
                        Some(jellyfin::BaseItemDto::from(media))
                    }
                    Err(e) => {
                        warn!("Failed to convert item to Media: {}", e);
                        None
                    }
                })
                .collect::<Vec<_>>();

            //let items = vec![];
            return Ok(ItemsQueryResult {
                items: items,
                total_count: 9999,
            });
        }
        //  }
    }

    // if q.filters.is_some() {
    //     return Ok(ItemsQueryResult {
    //         items: vec![],
    //         total_count: 0,
    //     });
    // }

    let manifest = aio.get_manifest().await?;

    if let Some(parent) = &parent {
        // catalog get
        if parent.kind == db::MediaKind::Catalog {
            // cataloga

            // if parent.promoted {
            q.parent_id = None;
            //}
            q.include_item_types =
                Some(vec![parent.catalog_media_kind_enum().unwrap().into()]);
            //             q.include_item_types = Some(vec![jellyfin::MediaType::Movie]);
           // trace!(?q, "CATALOG");

            let mut result =
                db::Media::get_by_jellyfin_filter(&state.ctx.db, &q, true).await?;

            return Ok(ItemsQueryResult {
                total_count: result.total_count as i64,
                items: result.records.into_vec(),
            });
        }

        //  }
    }
    //trace!(?q, "get_items");
    let mut result = db::Media::get_by_jellyfin_filter(&state.ctx.db, &q, false).await?;

    // handle details request
    if let Some(ids) = &q.ids {
        if ids.len() == 1 {
            let mut media: Option<db::Media> =
                if let Some(media) = result.records.get(0) {
                    Some(media.clone())
                } else {
                    if let Some(meta) =
                        state.ctx.store.get::<sdks::aio::Meta>(*ids.get(0).unwrap())
                    {
                        let mut media: db::Media = aio
                            .get_meta(meta.media_type.clone(), meta.id.clone())
                            .await?
                            .try_into()?;

                        // web client makes 2 simultenious request. So we get race conditions.
                        if let Err(err) = media.save(&state.ctx.db).await {
                            media = db::Media::get_by_filter(
                                &state.ctx.db,
                                &db::MediaFilter {
                                    aio_id: media.aio_id.clone(),

                                    ..Default::default()
                                },
                            )
                            .await?
                            .records
                            .get(0)
                            .unwrap()
                            .clone();
                        }

                        Some(media)
                    } else {
                        None
                    }
                };
            if let Some(media) = media.as_mut() {
                if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode)
                    && (q.fields.is_none()
                        || q.fields.as_ref().map_or(false, |f| {
                            f.contains(&jellyfin::ItemFields::MediaSources)
                        }))
                {
                    media.refresh_sources(&state.ctx.db, &state.ctx.aio).await?;
                    media.sources(&state.ctx.db).await?;

                    if let Some(sources) = &media.sources {
                        trace!(streams_len = sources.len(), "sources");
                    }
                }

                return Ok(ItemsQueryResult {
                    items: vec![media.clone().into()],
                    total_count: 1,
                });
            }
        }
    }

    Ok(ItemsQueryResult {
        items: result.records.into_vec(),
        total_count: 999_999,
    })
}

pub async fn items_flat(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let items = get_items(state, session, q, false).await?;
    Ok(Json::<Vec<jellyfin::BaseItemDto>>(items.items))
}

pub async fn items(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    //trace!(?q);
    let items = get_items(state, session, q.clone(), true).await?;

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i64,
        start_index: q.start_index.unwrap_or_else(|| 0),
        ..Default::default()
    }))
}

pub async fn item(
    state: AppState,
    session: auth::AuthSession,
    id: Uuid,
) -> Result<Option<jellyfin::BaseItemDto>> {
    let manifest = session.aio.get_manifest().await?;
    // let libraries = super::get_virtual_folders(&state).await?;

    // if let Some(library) = libraries.into_iter().find(|x| x.id == id) {
    //     return Ok(Some(library));
    // }

    let q = jellyfin::GetItemsQuery {
        ids: vec![id].into(),
        ..Default::default()
    };
    return Ok(get_items(state, session, q, false)
        .await?
        .items
        .first()
        .cloned());
}

pub async fn items_get(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    return Ok(Json(item(state, session, id).await?).into_response());
}

pub async fn users_items_get(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    return Ok(Json(item(state, session, id).await?).into_response());
}

pub async fn shows_seasons(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(mut q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    q.parent_id = Some(id);
    q.include_item_types = Some(vec![jellyfin::MediaType::Season]);
    let items = get_items(state, session, q.clone(), true).await?;

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        ..Default::default()
    }))
}

pub async fn shows_episodes(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(mut q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    // q.season_id = Some(id);
    q.parent_id = q.season_id;
    q.include_item_types = Some(vec![jellyfin::MediaType::Episode]);
    let items = get_items(state, session, q.clone(), true).await?;

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        // total_record_count: items.total_count as i64,
        // start_index: q.start_index.unwrap_or_else(|| 0),
        ..Default::default()
    }))
}

pub async fn user_image(
    State(state): State<AppState>,
    // Query(q): Query<jellyfin::ImageQuery>,
) -> Result<impl IntoResponse> {
    let url = Some("https://placehold.co/600x400".to_string());

    Ok(Redirect::temporary(url.unwrap().as_str()))
}

#[derive(Deserialize)]
enum ImageType {
    Primary,
}

#[derive(Deserialize)]
struct ImagePath {
    id: String,
    image_type: String,
    index: Option<usize>,
}

pub async fn items_images(
    State(state): State<AppState>,
    // session: auth::AuthSession,
    Path(ImagePath {
        id,
        image_type,
        index,
    }): Path<ImagePath>,
    Query(q): Query<jellyfin::ImageQuery>,
) -> Result<impl IntoResponse> {
    // trace!(%media_id.id, %image_type, ?index, ?q, "items_images");

    // we replace tags with urls so use that first.
    let mut url = q.tag;

    if url.is_none() {
        url = Some("https://placehold.co/600x400".to_string());
    }

    Ok(Redirect::temporary(url.unwrap().as_str()))
}

pub async fn items_playbackinfo(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    // Query(q): Query<jellyfin::PlaybackInfoQuery>,
    Json(payload): Json<jellyfin::PlaybackInfoQuery>,
) -> Result<impl IntoResponse> {
    trace!(?id, ?payload, "items_playbackinfo");

    //let mut item = session.item_store.get(&id);
    //let source = item.media_sources_mut(&session.aio)
    //        .await?
    //        .iter_mut()
    //        .find(|x| (q.media_source_id || x.id) == item.id)
    //        .expect("media source not found")
    //        .probe_in_place();

    //item.save(&session.item_store);
    let media = db::Media::get_by_id(&state.ctx.db, &payload.media_source_id.unwrap_or(id))
        .await?
        .context_not_found("not found", "not found")?;

    //let stream = id
    //    .stream
    //    .clone()
    //    .or_else(|| {
    //        payload
    //            .media_source_id
    //            .as_ref()
    //            .and_then(|m| m.stream.clone())
    //    })
    //    .context_not_found("not", "not")?;

    // let stream = session.aio.get_stream(
    //     id.jellyfin_media_type.into(),
    //     id.id.clone(),
    //     stream_id
    // ).await?;

    //let mut source: jellyfin::MediaSourceInfo =
    //    stream_into_media_source_info(id.id, id.jellyfin_media_type, stream);

    let mut source: jellyfin::MediaSourceInfo = media.into();
    source.probe_in_place()?;
    let subtitles: Vec<sdks::aio::Subtitle> = vec![];

    // info on tracks: https://github.com/jellyfin/Swiftfin/blob/main/Shared/Extensions/JellyfinAPI/MediaStream.swift#L219

    // Codec: "subrip" - Subtitle format codec (SRT)
    // TimeBase: "1/1000" - Time base in milliseconds
    // VideoRange: "Unknown" - Video color range (not applicable for subtitles)
    // VideoRangeType: "Unknown" - Color range type unknown
    // AudioSpatialFormat: "None" - No audio spatial format (subtitle)
    // LocalizedUndefined: "Undefined" - Label for undefined flag
    // LocalizedDefault: "Default" - Label for default flag
    // LocalizedForced: "Forced" - Label for forced subtitle flag
    // LocalizedExternal: "External" - Label for external subtitle source
    // LocalizedHearingImpaired: "Hearing Impaired" - Label for hearing impaired subtitles
    // DisplayTitle: "Undefined - SUBRIP - External" - Combined display title
    // IsInterlaced: false - Not interlaced (not applicable)
    // IsAVC: false - Not AVC video codec (subtitle)
    // IsDefault: false - Not default subtitle stream
    // IsForced: false - Not forced subtitle
    // IsHearingImpaired: false - Not hearing impaired subtitles
    // Height: 0 - Video height (not applicable)
    // Width: 0 - Video width (not applicable)
    // Type: "Subtitle" - Stream type is subtitle
    // Index: 0 - Stream index
    // IsExternal: true - Subtitle is external
    // DeliveryMethod: "External" - Delivered as external file
    // DeliveryUrl: "/Videos/657a70e0-ad75-82d8-2e64-c3a30c186a03/657a70e0ad7582d82e64c3a30c186a03/Subtitles/0/0/Stream.vtt?api_key=68068a69a1594bc1a1f34b394259630c" - URL for fetching subtitle stream
    // IsExternalUrl: false - DeliveryUrl is not an external URL
    // IsTextSubtitleStream: true - This is a text subtitle stream
    // SupportsExternalStream: true - External subtitle streams supported
    // Path: "/media/test/Ghosts.2021.S01E05.720p.AMZN.WEBRip.x264-GalaxyTV.srt" - Local file path for subtitle
    // Level: 0 - Subtitle level or priority

    let info = jellyfin::PlaybackInfoResponse {
        media_sources: vec![source],
        play_session_id: Some("test".to_string()),
        ..Default::default()
    };

    trace!(?info, "items_playbackinfo_result");
    Ok(Json(info))
}

pub async fn items_filters(State(state): State<AppState>) -> Result<impl IntoResponse> {
    /// genres is actually tags?
    use strum::IntoEnumIterator;
    // let genres = db::Genre::iter().map(|g| g.to_string()).collect();
    let current_year = chrono::Utc::now().year() as i64;
    //let years = (1900..=current_year).collect();

    Ok(Json(jellyfin::QueryFiltersLegacy {
        //  genres: Some(genres),
        //  years: Some(years),
        genres: None,
        years: None,
        ..Default::default()
    }))
}

/// Starts a direct playback for the given media UUID.
///
/// # Range
///
/// The `Range` header is forwarded to the upstream server. If no `Range` is provided,
/// the full video is sent.
///
/// # Static
///
/// If the `static_` query parameter is set to `true`, the response will be a static
/// video stream. Otherwise, a `jellyfin::PlaybackInfoResponse` is returned.
pub async fn videos_stream(
    // range: Option<axum_extra::TypedHeader<headers::Range>>,
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    //session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<jellyfin::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    //let (id, media_type, stream_id) = utils::decode_media_token(&uuid)?;
    //  .log_err("Failed to decode media UUID")

    let mut media = db::Media::get_by_id(&state.ctx.db, &q.media_source_id.unwrap_or(id))
        .await?
        .context_not_found("not found", "not found")?;

    if media.kind == db::MediaKind::Movie || media.kind == db::MediaKind::Episode {
        media = media
            .sources(&state.ctx.db)
            .await?
            .get(0)
            .context_not_found("not found", "not found")?
            .clone();
    }
    // trace!(?media, ?q, "videos_stream");

    // filter by id
    //let stream = id
    //    .stream
    //    .clone()
    //    .or_else(|| q.media_source_id.as_ref().and_then(|m| m.stream.clone()))
    //    .context_not_found("not", "not")?;

    //let stream = session.aio.get_stream(
    //    id.jellyfin_media_type.into(),
    //    id.id,
    //    stream_id
    //).await?;

    if q.static_.unwrap_or(false) {
        info!("starting direct playback for: {:?}", &media.title);
        let mut req = reqwest::Client::new().get(media.url.unwrap());
        // if let Some(axum_extra::TypedHeader(range)) = range {
        //     // let s = format!("{:?}", range);
        //     // req = req.header(axum::http::header::RANGE, s);
        //     req = req.header(http::header::RANGE, range);
        // }
        if let Some(v) = headers.get(http::header::RANGE) {
            req = req.header(http::header::RANGE, v.clone());
        }

        let upstream = req.send().await?;

        let status = upstream.status();
        let headers_in = upstream.headers().clone();
        let upstream_stream = upstream.bytes_stream();
        let body = Body::from_stream(upstream_stream.map_err(io::Error::other));

        trace!(?status, ?headers_in, "videos_stream");

        // Build outgoing response with same status
        let mut resp_out = axum::response::Response::builder()
            .status(status)
            .body(body)
            .unwrap();

        {
            use axum::http::header;
            let out_headers = resp_out.headers_mut();
            for (k, v) in headers_in.iter() {
                // Skip hop-by-hop headers
                match k.as_str().to_ascii_lowercase().as_str() {
                    "content-length" | "content-type" | "accept-ranges"
                    | "content-range" | "last-modified" => {}
                    _ => continue,
                }
                out_headers.insert(k, v.clone());
            }

            // If upstream didn’t set Content-Type, default to mp4 for static direct play
            if !out_headers.contains_key(header::CONTENT_TYPE) {
                out_headers.insert(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("video/mp4"),
                );
            }
        }

        return Ok(resp_out);
    }

    todo!();
}

pub async fn users_me(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::UserDto::from(session.user)).into_response())
}

/// todo: actually @molement
pub async fn playback_bitratetest(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn users_groupingoptions(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json::<Vec<jellyfin::SpecialViewOptionDto>>(vec![]))
}

pub async fn stub(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn stub_json(State(state): State<AppState>) -> Result<impl IntoResponse> {
    //let user: UserDtoDummy = Faker.fake();
    //Ok(Json().into_response())
    Ok(Json(json!({
      "ThemeVideosResult": {
        "OwnerId": "f27caa37e5142225cceded48f6553502",
        "Items": [],
        "TotalRecordCount": 0,
        "StartIndex": 0
      },
      "ThemeSongsResult": {
        "OwnerId": "f27caa37e5142225cceded48f6553502",
        "Items": [],
        "TotalRecordCount": 0,
        "StartIndex": 0
      },
      "SoundtrackSongsResult": {
        "OwnerId": "00000000000000000000000000000000",
        "Items": [],
        "TotalRecordCount": 0,
        "StartIndex": 0
      }
    }))
    .into_response())
    // match media::Entity::find_by_id(id).one(&state.conn).await? {
    //     Some(item) => {
    //         Ok(Json(jellyfin_sdk::types::BaseItemDto::from(item)).into_response())
    //    }
    //    None => Ok(StatusCode::NOT_FOUND.into_response()),
    // }
}

pub async fn mock_items(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        ..Default::default()
    }))
}
//fn id_encode
