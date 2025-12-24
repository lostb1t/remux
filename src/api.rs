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
use chrono;
use http::StatusCode;
use std::str::FromStr;
use anyhow::Context;
use serde::Deserialize;
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;
//use bytes::Bytes;
use axum_extra::response::file_stream::FileStream;
use futures_util::StreamExt;
use futures_util::stream::Stream;
use std::convert::Infallible;
use std::io;
use tokio_util::io::ReaderStream;
use tokio_util::io::StreamReader;
use tracing::info;
use crate::AuthState;
use anyhow::anyhow;
use crate::errors::LogErr;
use crate::sdks::jellyfin::MediaSourceInfo;
use crate::sdks::jellyfin::MediaStream;
use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use ffprobe;
use futures::future::join_all;
use futures_util::TryStreamExt;
use headers;
use serde_json::json;
use tracing::trace;
use tracing::warn;

//use crate::db;
//use anyhow::Result;
use axum_anyhow::{ApiResult as Result, OptionExt};
use crate::rewrite_request_uri;
use crate::sdks;
use crate::sdks::{jellyfin, aio, tmdb};
use crate::utils;
use crate::utils::server_id;
use crate::AppState;
use chrono::Datelike;
use tower::util::MapRequestLayer;


pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users/authenticatebyname", post(users_authenticatebyname))
        .route("/system/info/public", get(system_info_public))
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
        .route("/users/{id}", get(users_me))
        .route("/users/{user_id}/items", get(items))
        .route("/users/{user_id}/items/latest", get(items_flat))
        .route("/users/{user_id}/items/{id}", get(users_items_get))
        .route("/users/{id}/views", get(userviews))
        .route("/users/{id}/groupingoptions", get(users_groupingoptions))
        .route("/videos/{id}/stream", get(videos_stream))
        .route("/playback/bitratetest", get(playback_bitratetest))
        .route("/displaypreferences/usersettings", get(user_settings))
        .route("/system/info", get(system_info))
        // stubs. to implement
        .route("/shows/nextup", get(mock_items))
        .route("/users/{user_id}/items/resume", get(mock_items))
        .route("/users/{user_id}/items/similar", get(mock_items))
        .route("/users/{user_id}/intros", get(mock_items))
        .route("/users/{user_id}/items/{id}/intros", get(mock_items))
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

    //.map_request(rewrite_request_uri)
    //.layer(MapRequestLayer::new(rewrite_request_uri))
    // .route("/jellyfin/Items/{id}/Image/{image_type}", get_service(ServeFile::new("assets/placeholder_poster.jpg")))
}

pub fn test_media_source() -> jellyfin::MediaSourceInfo {
    jellyfin::MediaSourceInfo {
           id: Some("test".to_string()),
           name: Some("test gues test yes".to_string()),
           ///type_: Some(jellyfin::types::MediaSourceType::Video),
           //type_: Some(jellyfin::types::BaseItemKind::Movie),
           media_streams: Some(vec![
             jellyfin::MediaStream {
            // id: Some("1234".to_string()),
            // name: Some("test".to_string()),
             type_: Some(jellyfin::MediaStreamType::Video),
             ..Default::default()
             },
             jellyfin::MediaStream {
             index: Some(0),
             display_title: Some("test".to_string()),
             type_: Some(jellyfin::MediaStreamType::Subtitle),
             ..Default::default()
           }
            ]),
           ..Default::default()
         }
}

pub fn test_items() -> Vec<jellyfin::BaseItemDto> {
    vec![jellyfin::BaseItemDto {
        id: "tt2294629".to_string(),
        name: Some("test".to_string()),
        type_: Some(sdks::jellyfin::MediaType::Movie),
        //original_title: Some("yogo".to_string()),
        media_sources: Some(vec![test_media_source()]),
        ..Default::default()
    }]
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

pub async fn user_settings(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::DisplayPreferencesDto {
        id: Some("test".to_string()),
        ..Default::default()
    }))
}

pub async fn users_authenticatebyname(
    State(state): State<AppState>,
    Json(data): Json<jellyfin::AuthenticateUserByName>,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::AuthenticationResult {
        access_token: Some("sometoken".to_string()),
        server_id: Some(server_id()),
        user: Some(jellyfin::UserDto {
            server_id: Some(server_id()),
            name: Some("test".to_string()),
            id: Some(1.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

/// This sbould hold dynamic collections
pub async fn userviews(State(state): State<AppState>) -> Result<impl IntoResponse> {
    //let mut items: Vec<jellyfin::BaseItemDto> = state.stremio.get_catalogs().await?.into_iter().map(jellyfin::BaseItemDto::from).collect();
    //items.extend(utils::libraries());
    let items = utils::libraries();
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items,
        ..Default::default()
    }))
}

pub async fn userviews_groupingoptions(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json(json!(utils::libraries())))
}

pub async fn library_virtualfolders(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json(json!(utils::libraries())))
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
        items: test_items(),
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
    auth: AuthState,
    mut q: jellyfin::GetItemsQuery,
    _count: bool,
) -> Result<ItemsQueryResult> {
    let aio = auth.user.get_aio()?;
    let aio_search = auth.user.get_aio_search()?;
    let search = q.search_term.clone().or(q.name_starts_with.clone());
    let skip = q.start_index.unwrap_or(0) as u32;

    // only support Movie and Series for search and catalogs
    if search.is_some()
        || q.parent_id
            .as_deref()
            .map_or(false, |id| id.starts_with("catalog"))
    {
        if let Some(types) = &q.include_item_types {
            if ![jellyfin::MediaType::Movie, jellyfin::MediaType::Series].contains(&types[0]) {
                return Ok(ItemsQueryResult {
                    items: vec![],
                    total_count: 0,
                });
            }
        }
    }

    if q.filters.is_some() {
        return Ok(ItemsQueryResult {
            items: vec![],
            total_count: 0,
        });
    }

let manifest = aio
    .execute(&aio::ManifestEndpoint)
    .await?;

    // helper: find catalog by uuid (catalog:<uuid>)
    let find_catalog_by_uuid = |m: &aio::Manifest, uuid: &str| -> Option<aio::Catalog> {
        manifest.catalogs.iter().find(|c| c.uuid == uuid).cloned()
    };

    // helper: meta
    let fetch_meta = |media_type: aio::MediaType, imdb_id: String| async {
        aio
            .execute(&aio::MetaEndpoint {
                media_type,
                id: imdb_id,
                season: None,
                episode: None,
            })
            .await
            .map(|r| r.meta)
    };


    // helper: streams (search endpoint)
    let fetch_streams = |kind: aio::MediaType, id: String| async {
        auth.user
            .get_aio_search()?
            .execute(&aio::Search {
                kind,
                id,
                format: true,
            })
            .await
            .map(|r| r.data.results)
    };

    // ------------------------------------------------------------
    // Parent-based behavior
    // ------------------------------------------------------------

    if let Some(parent_id) = &q.parent_id {
        // "collections" = list catalogs
        if parent_id == "collections" {

            let items: Vec<jellyfin::BaseItemDto> = manifest
                .catalogs
                .into_iter()
                .map(jellyfin::BaseItemDto::from)
                .collect();

            return Ok(ItemsQueryResult {
                total_count: items.len() as i64,
                items,
            });
        }

        // catalog browsing by uuid ("catalog:<uuid>")
        if parent_id.starts_with("catalog:") {
            let catalog = find_catalog_by_uuid(&manifest, parent_id)
                .ok_or_else(|| anyhow!("catalog not found"))?;

            if let Some(types) = &q.include_item_types {
                let wanted: aio::MediaType = types[0].into();
                if catalog.kind != wanted {
                    return Ok(ItemsQueryResult {
                        items: vec![],
                        total_count: 0,
                    });
                }
            }


            let metas = aio
            .execute(&aio::CatalogEndpoint {
                kind: catalog.kind.clone(),
                id: catalog.id.clone(),
                search: search.clone(),
                genre: None,
                skip: Some(skip)
            })
            .await
            .map(|r| r.metas)?;

            // return Ok(ItemsQueryResult {
            //     kind: catalog.kind.clone(),
            //     id: catalog.id.clone(),
            //     search: search.clone(),
            //     genre,
            //     skip,
            // })
            // .await
            // .map(|r| r.metas)?;

            return Ok(ItemsQueryResult {
                items: metas.into_iter().map(jellyfin::BaseItemDto::from).collect(),
                total_count: 9_999_999,
            });
        }

        // seasons listing under a show id
        if let Some(types) = &q.include_item_types {
            if types[0] == jellyfin::MediaType::Season {
                let (id, media_type, _stream_id) =
                    utils::decode_media_uuid(parent_id)?;
                    //.log_err("Failed to decode media UUID")?;

                let meta = fetch_meta(media_type.into(), id.clone()).await?;

                let seasons: Vec<jellyfin::BaseItemDto> = meta
                    .get_season_numbers()
                    .into_iter()
                    .map(|x| jellyfin::BaseItemDto {
                        id: utils::encode_media_uuid(
                            format!("{}:{}", id.clone(), x).as_str(),
                            jellyfin::MediaType::Season,
                            None,
                        ),
                        name: Some(format!("Season {}", x)),
                        index_number: Some(x as i32),
                        ..Default::default()
                    })
                    .collect();

                return Ok(ItemsQueryResult {
                    total_count: seasons.len() as i64,
                    items: seasons,
                });
            }
        }
    }

    // episodes listing under a show id (optionally season_id present)
    if let Some(parent_id) = q.parent_id.clone() {
        if let Some(types) = &q.include_item_types {
            if types[0] == jellyfin::MediaType::Episode {
                let (id, _media_type, _stream_id) =
                    utils::decode_media_uuid(&parent_id)?;
                    //.log_err("Failed to decode media UUID")?;

                let mut season_number: Option<i32> = None;
                if let Some(season_id) = q.season_id.take() {
                    let (decoded, _, _) = utils::decode_media_uuid(&season_id)?;
                       // .context("Failed to decode media UUID")?;
                    let (_, sn) = decoded
                        .rsplit_once(':')
                        .ok_or_else(|| anyhow!("invalid season id"))?;
                    season_number = Some(sn.parse::<i32>()?);
                }

                let meta = fetch_meta(aio::MediaType::Series, id.clone()).await?;

                let episodes: Vec<jellyfin::BaseItemDto> = meta
                    .videos
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|e| season_number.map(|s| e.season == Some(s)).unwrap_or(false))
                    .map(jellyfin::BaseItemDto::from)
                    .collect();

                return Ok(ItemsQueryResult {
                    total_count: episodes.len() as i64,
                    items: episodes,
                });
            }
        }
    }

    // request for a single catalog (ids[0] is "catalog:<uuid>")
    if let Some(ids) = &q.ids {
        if ids[0].starts_with("catalog:") {
            let catalog = find_catalog_by_uuid(&manifest, &ids[0])
                .ok_or_else(|| anyhow!("catalog not found"))?;
            return Ok(ItemsQueryResult {
                items: vec![catalog.into()],
                total_count: 1,
            });
        }
    }

    // ------------------------------------------------------------
    // Single item (ids=...)
    // ------------------------------------------------------------
    if let Some(ids) = &q.ids {
        let uuid = &ids[0];
        let (id, media_type, stream_id) =
            utils::decode_media_uuid(uuid).unwrap();
            //.log_err("Failed to decode media UUID")?;

        if ![jellyfin::MediaType::Movie, jellyfin::MediaType::Series].contains(&media_type) {
            return Ok(ItemsQueryResult {
                items: vec![],
                total_count: 0,
            });
        }

        let meta = fetch_meta(media_type.into(), id.clone()).await?;
        let mut item: jellyfin::BaseItemDto = meta.into();

        if stream_id.is_some() {
            item.id = uuid.to_string();
        }

        let imdb = item
            .provider_ids
.as_ref()
    .ok_or_else(|| anyhow!("providerids missing"))?
            .imdb.clone().unwrap();

        let streams = fetch_streams(media_type.into(), imdb).await?;

        item.media_sources = Some(
            streams
                .into_iter()
                .filter(|x| stream_id.clone().map_or(true, |sid| x.id() == sid))
                .map(|stream| {
                    let mut source: jellyfin::MediaSourceInfo = stream.clone().into();
                    let enc =
                        utils::encode_media_uuid(&id, media_type, Some(stream.id()));
                    source.id = Some(enc.clone());
                    source.e_tag = Some(enc);
                    source
                })
                .collect::<Vec<jellyfin::MediaSourceInfo>>(),
        );

        return Ok(ItemsQueryResult {
            items: vec![item],
            total_count: 1,
        });
    }

    // ------------------------------------------------------------
    // Default browsing (no ids): pick catalog, then list items
    // ------------------------------------------------------------


    // choose catalog:
    // - if parent_id looks like "movie"/"series", use that as desired kind
    // - else if include_item_types provided, use that kind
    // - else just first catalog
    let desired_kind: Option<aio::MediaType> = q
        .parent_id
        .as_deref()
        .and_then(|pid| aio::MediaType::from_str(pid).ok())
        .or_else(|| {
            q.include_item_types
                .as_ref()
                .and_then(|v| v.first())
                .cloned()
                .map(Into::into)
        });

    let catalog = if let Some(kind) = desired_kind {
        manifest.catalogs
            .iter()
            .find(|c| c.kind == kind)
            .cloned()
            .or_else(|| manifest.catalogs.first().cloned())
            .ok_or_else(|| anyhow!("no catalogs"))?
    } else {
        manifest.catalogs.first().cloned().ok_or_else(|| anyhow!("no catalogs"))?
    };

            let metas = aio
            .execute(&aio::CatalogEndpoint {
                kind: catalog.kind.clone(),
                id: catalog.id.clone(),
                search: search.clone(),
                genre: None,
                skip: Some(skip)
            })
            .await
            .map(|r| r.metas)?;

    let items: Vec<jellyfin::BaseItemDto> = metas
        .into_iter()
        .map(jellyfin::BaseItemDto::from)
        .collect();

    Ok(ItemsQueryResult {
        items,
        total_count: 999_999,
    })
}

pub async fn items_flat(
    State(state): State<AppState>,
    auth: AuthState,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let items = get_items(state, auth, q, false).await?;
    Ok(Json::<Vec<jellyfin::BaseItemDto>>(items.items))
}

pub async fn items(
    State(state): State<AppState>,
    auth: AuthState,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let items = get_items(state, auth, q.clone(), true).await?;

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i32,
        start_index: q.start_index.unwrap_or_else(|| 0),
        ..Default::default()
    }))
}

pub async fn item(
    state: AppState,
    auth: AuthState,
    id: String,
) -> Result<Option<jellyfin::BaseItemDto>> {
    if let Some(library) = utils::libraries().into_iter().find(|x| x.id.clone() == id) {
        return Ok(Some(library));
    }

    let q = jellyfin::GetItemsQuery {
        ids: vec![id].into(),
        ..Default::default()
    };
    return Ok(get_items(state, auth, q, false).await?.items.first().cloned());
}

pub async fn items_get(
    State(state): State<AppState>,
        auth: AuthState,
    Path(id): Path<String>,
) -> Result<impl IntoResponse> {
    return Ok(Json(item(state, auth, id).await?).into_response());
}

pub async fn users_items_get(
    State(state): State<AppState>,
        auth: AuthState,
    Path((user_id, id)): Path<(String, String)>,
) -> Result<impl IntoResponse> {
    return Ok(Json(item(state, auth, id).await?).into_response());
}

pub async fn shows_seasons(
    State(state): State<AppState>,
    auth: AuthState,
    Path(id): Path<String>,
    Query(mut q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    q.parent_id = Some(id);
    q.include_item_types = Some(vec![jellyfin::MediaType::Season]);
    let items = get_items(state, auth, q.clone(), true).await?;

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        ..Default::default()
    }))
}

pub async fn shows_episodes(
    State(state): State<AppState>,
        auth: AuthState,
    Path(id): Path<String>,
    Query(mut q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    // q.season_id = Some(id);
    q.parent_id = Some(id);
    q.include_item_types = Some(vec![jellyfin::MediaType::Episode]);
    let items = get_items(state, auth, q.clone(), true).await?;

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        // total_record_count: items.total_count as i32,
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
            auth: AuthState,
    Path(ImagePath {
        id,
        image_type,
        index,
    }): Path<ImagePath>,
    Query(q): Query<jellyfin::ImageQuery>,
) -> Result<impl IntoResponse> {
    trace!(%id, %image_type, ?index, ?q, "items_images");
    // we replace tags with urls so use that first.
    let mut url = q.tag;
    // dbg!(&url, "items_images");
    if url.is_none() {
        // first decode the id and type
        let (id, mut media_type, stream_id) =
            utils::decode_media_uuid(&id)?;
            //.log_err("Failed to decode media UUID")?;

        let ids: Vec<String> = id.split(':').map(|s| s.to_string()).collect();

        let mut base_media_type = media_type;
        if media_type == jellyfin::MediaType::Episode
            || media_type == jellyfin::MediaType::Season
        {
            base_media_type = jellyfin::MediaType::Series;
        }
        // dbg!(&ids, media_type, "items_images");

        // get details

        let meta = auth.user
            .get_aio()?
            .execute(&aio::MetaEndpoint {
                media_type: base_media_type.into(),
                id: ids[0].clone(),
                ..Default::default()
            })
            .await
            .map(|r| r.meta)?;

        // dbg!(&meta);
        // return images.
        if media_type == jellyfin::MediaType::Episode {
            dbg!(meta.get_episode_by_id(id.clone()));
            url = meta.get_episode_by_id(id).unwrap().thumbnail.clone();
        }

        if url.is_none() && image_type == "primary" && meta.poster.is_some() {
            url = meta.poster;
        }
    };

    if url.is_none() {
        url = Some("https://placehold.co/600x400".to_string());
    }

    Ok(Redirect::temporary(url.unwrap().as_str()))
}

pub async fn items_playbackinfo(
    State(state): State<AppState>,
    auth: AuthState,
    Path(id): Path<String>,
    Query(query): Query<jellyfin::PlaybackInfoQuery>,
    Json(payload): Json<jellyfin::PlaybackInfoQuery>,
) -> Result<impl IntoResponse> {
    let (id, media_type, stream_id) = utils::decode_media_uuid(&id)
   //     .log_err("Failed to decode media UUID")
        .unwrap();

    trace!(?id, ?media_type, ?stream_id, ?payload, ?query, "items_playbackinfo");

    /// todo: media source can also be send as streamid.
    let media_source_id: Option<String> = payload
        .media_source_id
        .clone()
        .or_else(|| query.media_source_id.clone())
        .and_then(|s| {
            let (_, _, stream_id) = utils::decode_media_uuid(&s)
          //      .log_err("Failed to decode media UUID")
                .unwrap();
            stream_id
        });



    let filter_by_id: Option<String> = stream_id.or(media_source_id.clone());
   // dbg!(&filter_by_id, "filter_by_id");
  // let remux = Remux::from_user(&auth.user);
   let mut streams: Vec<sdks::aio::Stream> =
    auth.user
        .get_aio_search()?
        .execute(&aio::Search {
            kind: media_type.into(),
            id,
            format: true,
        })
        .await
        .map(|r| r.data.results)?
        //.map_err(Into::into)?
        .into_iter()
        .filter(|x| {
            filter_by_id
                .clone()
                .map(|id| x.id() == *id)
                .unwrap_or(true)
        })
        .collect();
   // dbg!(&streams);
    // fallback
    streams = vec![streams[0].clone()];
    // dbg!(&streams);

    // let subtitles = state
    //     .stremio
    //     .get_subtitles(id, media_type.into(), None, None)
    //     .await?;
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

    let mut media_sources: Vec<MediaSourceInfo> = streams
        .into_iter()
        .map(|stream| {
            let mut media_source: MediaSourceInfo = stream
                .probe()
                .log_err("Failed to probe media stream")?
                .into();
            let subtitles = subtitles.clone();

            if let Some(media_streams) = media_source.media_streams.as_mut() {
                media_streams.extend({
                    subtitles
                        .into_iter()
                        .map(|subtitle| subtitle.into())
                        .collect::<Vec<_>>()
                });
            }

            Ok(media_source)
        })
        .collect::<Result<Vec<_>>>()?;

    // let mut media_sources: Vec<MediaSourceInfo> = streams
    //     .into_iter()
    //     .map(|stream| {
    //         let mut media_source: MediaSourceInfo = stream.probe().log_err("Failed to probe media stream")?.into();
    //         let subtitles = subtitles.clone();

    //         if let Some(media_streams) = media_source.media_streams.as_mut() {
    //             media_streams.extend({
    //                 subtitles
    //                     .into_iter()
    //                     .map(|subtitle| subtitle.into())
    //                     .collect::<Vec<_>>()
    //             });
    //         }

    //         Ok(media_source)
    //     })
    //     .collect::<Result<Vec<_>>>()?;

    // media_sources.extend({
    //     subtitles
    //         .into_iter()
    //         .map(|subtitle| subtitle.into())
    // });

    let info = jellyfin::PlaybackInfoResponse {
        media_sources,
        // media_sources: streams
        //     .into_iter()
        //     .map(|stream| stream.probe().unwrap().into())
        //     .collect(),
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
    let current_year = chrono::Utc::now().year() as i32;
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
    auth: AuthState,
    Path(uuid): Path<String>,
    Query(q): Query<jellyfin::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    let (id, media_type, stream_id) = utils::decode_media_uuid(&uuid)?;
      //  .log_err("Failed to decode media UUID")

    trace!(?uuid, ?q, ?id, ?headers, ?stream_id, "videos_stream");

    let streams = auth
        .user
        .get_aio_search()?
        .execute(
        &aio::Search {
          kind: media_type.into(),
          id: id,
          ..Default::default()
        }
        )
      //  .get_streams(id, media_type.into(), None, None)
        .await
        .unwrap();

    // filter by id
    let filter_by_id: Option<String> = stream_id.or(q.media_source_id.clone());
    let stream = streams
        .data
        .results
        .into_iter()
        .find(|x| {
            filter_by_id
                .as_ref()
                .map(|id| x.id() == *id)
                .unwrap_or(true)
        })
        .context_not_found("no stream", "no stream")?;

    if q.static_.unwrap_or(false) {
        info!("starting direct playback for: {:?}", &stream.name);
        let mut req = reqwest::Client::new().get(stream.url.unwrap());
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

pub async fn users_me(State(state): State<AppState>) -> Result<impl IntoResponse> {
    //let user: UserDtoDummy = Faker.fake();
    Ok(Json(jellyfin::UserDto {
        id: Some("test".to_string()),
        name: Some("test".to_string()),
        has_password: Some(true),
        server_id: Some(server_id()),
        ..Default::default()
    })
    .into_response())
    //Ok(StatusCode::NOT_FOUND.into_response())
    // match media::Entity::find_by_id(id).one(&state.conn).await? {
    //     Some(item) => {
    //         Ok(Json(jellyfin_sdk::types::BaseItemDto::from(item)).into_response())
    //    }
    //    None => Ok(StatusCode::NOT_FOUND.into_response()),
    // }
}

/// todo: actually @molement
pub async fn playback_bitratetest(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    //let user: UserDtoDummy = Faker.fake();
    //Ok(Json().into_response())
    Ok(StatusCode::NO_CONTENT.into_response())
    // match media::Entity::find_by_id(id).one(&state.conn).await? {
    //     Some(item) => {
    //         Ok(Json(jellyfin_sdk::types::BaseItemDto::from(item)).into_response())
    //    }
    //    None => Ok(StatusCode::NOT_FOUND.into_response()),
    // }
}

pub async fn users_groupingoptions(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json::<Vec<jellyfin::SpecialViewOptionDto>>(vec![]))
}
//fn id_encode

pub async fn stub(State(state): State<AppState>) -> Result<impl IntoResponse> {
    //let user: UserDtoDummy = Faker.fake();
    //Ok(Json().into_response())
    Ok(StatusCode::NO_CONTENT.into_response())
    // match media::Entity::find_by_id(id).one(&state.conn).await? {
    //     Some(item) => {
    //         Ok(Json(jellyfin_sdk::types::BaseItemDto::from(item)).into_response())
    //    }
    //    None => Ok(StatusCode::NOT_FOUND.into_response()),
    // }
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
