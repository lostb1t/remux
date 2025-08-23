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
//use axum_route::route;
use chrono;
use eyre;
use http::StatusCode;
use sea_orm::ColIdx;
use std::str::FromStr;
//use crate::sdks::tmdb;
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
//use futures_util::StreamExt;
use crate::errors::LogErr;
use crate::sdks::jellyfin::MediaSourceInfo;
use crate::sdks::jellyfin::MediaStream;
use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use ffprobe;
use futures::future::join_all;
use futures_util::TryStreamExt;
use headers;
use sea_orm::ColumnTrait;
use sea_orm::EntityTrait;
use sea_orm::Order;
use sea_orm::PaginatorTrait;
use sea_orm::QueryFilter;
use sea_orm::QueryOrder;
use sea_orm::QuerySelect;
use sea_orm::RelationTrait;
use serde_json::json;
use tracing::trace;
use tracing::warn;

use crate::db;
use crate::errors::Result;
use crate::rewrite_request_uri;
use crate::sdks;
use crate::sdks::{jellyfin, stremio, tmdb};
use crate::utils;
use crate::utils::server_id;
use chrono::Datelike;
use tower::util::MapRequestLayer;
//use crate::AppState;

#[derive(Debug, Clone)]
pub struct AppState {
    pub config: crate::Config,
    pub db: db::Database,
    pub tmdb: sdks::core::RestClient,
    pub stremio: sdks::stremio::StremioService,
}

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

pub async fn get_items_query_conditions(
    state: AppState,
    q: jellyfin::GetItemsQuery,
) -> Result<sea_orm::Condition> {
    let mut conditions = sea_orm::Condition::all();

    if let Some(name_start) = &q.name_starts_with {
        conditions =
            conditions.add(db::media::Column::Name.like(format!("{}%", name_start)));
    }

    if let Some(search_term) = &q.search_term {
        conditions = conditions
            .add(db::media::Column::Name.contains(format!("{}", search_term)));
    }

    if let Some(ids) = &q.ids {
        conditions = conditions.add(db::media::Column::Id.is_in(ids.clone()));
    }

    if let Some(genres) = &q.genres {
        // conditions = conditions.add(db::media_genre::Column::Genre.is_in(genres.clone()));
    }

    if let Some(years) = &q.years {
        let mut cond = sea_orm::Condition::any(); // OR across years

        for &year in years {
            let start_date = chrono::NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
            let end_date = chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap();

            let year_cond = db::media::Column::ReleaseDate
                .gte(start_date)
                .and(db::media::Column::ReleaseDate.lt(end_date));

            cond = cond.add(year_cond);
        }

        conditions = conditions.add(cond);
    }

    if let Some(types) = &q.include_item_types {
        //conditions = conditions.add(db::media::Column::MediaType.is_in(types.clone()));
    }

    if let Some(season_id) = &q.season_id {
        conditions = conditions.add(db::media::Column::ParentId.eq(season_id.clone()));
    }

    if let Some(parent_id) = &q.parent_id {
        if parent_id == "movies" {
            conditions = conditions
                .add(db::media::Column::MediaType.eq(db::media::MediaType::Movie));
        } else if parent_id == "series" {
            conditions = conditions
                .add(db::media::Column::MediaType.eq(db::media::MediaType::Series));
        } else if parent_id.starts_with("catalog") {
            //let catalogs = state.stremio.get_catalogs().await?;
            // let catalog = state.stremio.addons
            //     .into_iter()
            //     .find(|x| {
            //       x.manifest.catalogs.iter().find(|y| &x.catalog_guid(y) == parent_id)
            //     })
            //     .expect("catalog not found");
            //if let Some(catalog) = state.stremio.get_catalog(parent_id) {
            //   conditions = conditions.add(db::media::Column::ImdbId.is_in(ids));
            //}

            // let ids: Vec<_> = catalog
            //     .get_items().await
            //     .unwrap_or_default()
            //     .into_iter()
            //     .map(|x| x.id)
            //     .collect();
            //  let ids: Vec<_> = vec![];

            //   conditions = conditions.add(db::media::Column::ImdbId.is_in(ids));
        } else {
            conditions =
                conditions.add(db::media::Column::ParentId.eq(parent_id.clone()));
        }
    }

    Ok(conditions)
}

pub fn apply_sorting(
    mut query: sea_orm::Select<db::media::Entity>,
    q: jellyfin::GetItemsQuery,
) -> sea_orm::Select<db::media::Entity> {
    use db::media::Column as MediaColumn;
    use sea_orm::sea_query::Expr;

    let order: sea_orm::Order = q
        .sort_order
        .unwrap_or(jellyfin::SortOrder::Ascending)
        .into();
    if let Some(sort_by_vec) = &q.sort_by {
        for sort_by in sort_by_vec {
            query = match sort_by {
                jellyfin::ItemSortBy::SortName => {
                    query.order_by(MediaColumn::Name, order.clone())
                }
                jellyfin::ItemSortBy::Name => {
                    query.order_by(MediaColumn::Name, order.clone())
                }
                jellyfin::ItemSortBy::PremiereDate => {
                    query.order_by(MediaColumn::ReleaseDate, order.clone())
                }
                jellyfin::ItemSortBy::Random => {
                    query.order_by_asc(Expr::cust("RANDOM()"))
                }
                _ => query.order_by(MediaColumn::Id, order.clone()),
            };
        }
    }

    query
}

pub struct ItemsQueryResult {
    pub items: Vec<jellyfin::BaseItemDto>,
    pub total_count: i64,
}

pub async fn get_items(
    state: AppState,
    mut q: jellyfin::GetItemsQuery,
    count: bool,
) -> Result<ItemsQueryResult> {
    // trace!(&q);
    // dbg!(&q);

    let search = q.search_term.clone().or(q.name_starts_with.clone()); // for now, dont do a few requests

    // only support Movie and Series for sesrch and catalogs
    if search.is_some()
        || q.parent_id
            .as_deref()
            .map_or(false, |id| id.starts_with("catalog"))
    {
        if let Some(types) = &q.include_item_types {
            if ![jellyfin::MediaType::Movie, jellyfin::MediaType::Series]
                .contains(&types[0])
            {
                return Ok(ItemsQueryResult {
                    items: vec![],
                    total_count: 0,
                });
            }
        }
    }

    if q.filters.is_some() {
        return Ok(ItemsQueryResult {
            // items: items,
            items: vec![],
            total_count: 0,
        });
    }

    let skip = q.start_index.unwrap_or_else(|| 0) as u32;

    if let Some(parent_id) = &q.parent_id {
        if parent_id == "collections" {
            let items: Vec<jellyfin::BaseItemDto> = state
                .stremio
                .get_catalogs()
                .into_iter()
                .map(jellyfin::BaseItemDto::from)
                .collect();
            return Ok(ItemsQueryResult {
                total_count: items.len() as i64,
                items,
            });
        }

        // get datalog items
        if parent_id.starts_with("catalog") {
            if let Some(types) = &q.include_item_types {
                let catalog = state
                    .stremio
                    .get_catalog(parent_id.clone().as_str())
                    .unwrap();
                if catalog.kind != types[0].into() {
                    return Ok(ItemsQueryResult {
                        items: vec![],
                        total_count: 0,
                    });
                }
            }
            let items = state
                .stremio
                .get_catalog_items(parent_id.clone(), None, q.limit, Some(skip))
                .await
                .unwrap();
            return Ok(ItemsQueryResult {
                items: items.into_iter().map(jellyfin::BaseItemDto::from).collect(),
                total_count: 9999999,
            });
        }

        if let Some(types) = &q.include_item_types {
            if types[0] == jellyfin::MediaType::Season {
                // utils::encode_media_uuid(&parent_id).unwrap()
                let (id, media_type, stream_id) = utils::decode_media_uuid(&parent_id)
                    .log_err("Failed to decode media UUID")
                    .unwrap();
                let seasons: Vec<jellyfin::BaseItemDto> = state
                    .stremio
                    .get_meta(id.clone(), media_type.into(), None, None)
                    .await
                    .unwrap()
                    .unwrap()
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
                // dbg!("YOOOOO");
                // dbg!(&seasons);
                return Ok(ItemsQueryResult {
                    total_count: seasons.len() as i64,
                    items: seasons,
                });
            }
        }
        // }
    }

    // Get episodes
    if let Some(parent_id) = q.parent_id.clone() {
        if let Some(types) = &q.include_item_types {
            if types[0] == jellyfin::MediaType::Episode {
                // dbg!(&parent_id);
                let (id, media_type, stream_id) = utils::decode_media_uuid(&parent_id)
                    .log_err("Failed to decode media UUID")?;
                // dbg!(&id, &media_type);
                let mut season_number: Option<i32> = None;
                if let Some(season_id) = q.season_id {
                    // let (id, _season_number) = season_id.rsplit_once(':').unwrap();
                    let (id, _, _) = utils::decode_media_uuid(&season_id)
                        .log_err("Failed to decode media UUID")?;
                    // dbg!(&id);
                    let (_, _season_number) = id.rsplit_once(':').unwrap();
                    season_number = Some(_season_number.parse::<i32>().unwrap());
                }

                // dbg!(&id);
                // dbg!(&id);
                let episodes: Vec<jellyfin::BaseItemDto> = state
                    .stremio
                    .get_meta(
                        id.clone().to_string(),
                        sdks::stremio::MediaType::Series,
                        None,
                        None,
                    )
                    .await
                    .unwrap()
                    .unwrap()
                    .videos
                    .unwrap()
                    .into_iter()
                    .filter(|x| {
                        if season_number.is_some() {
                            return x.season.unwrap() == season_number.unwrap();
                        }
                        false
                    })
                    .map(jellyfin::BaseItemDto::from)
                    .collect();
                // dbg!("YOOOOO");
                // dbg!(&seasons);
                return Ok(ItemsQueryResult {
                    total_count: episodes.len() as i64,
                    items: episodes,
                });
            }
        }
    }

    // request for a single catalog
    if let Some(ids) = &q.ids {
        if ids[0].starts_with("catalog") {
            let catalog = state.stremio.get_catalog(ids[0].as_str()).unwrap();
            // let catalog = catalogs
            //              .into_iter()
            //              .find(|x| x.guid() == ids[0])
            //               .expect("catalog not found");
            return Ok(ItemsQueryResult {
                items: vec![catalog.into()],
                total_count: 1,
            });
        }
    }

    //let endpoint = match
    //1sdks::tmdb::Movie::
    //let res = state.tmdb
    //let endpoint = sdks::tmdb::Movie::Discover

    let mut items: Vec<jellyfin::BaseItemDto> = vec![];

    // single item. We assume details
    if let Some(ids) = &q.ids {
        let uuid = &ids[0];
        let (id, media_type, stream_id) = utils::decode_media_uuid(&uuid)
            .log_err("Failed to decode media UUID")
            .unwrap();
        // dbg!(&id, &media_type, &stream_id);
        // just support movie and series for now
        // todo: add support for episodes.
        if ![jellyfin::MediaType::Movie, jellyfin::MediaType::Series]
            .contains(&media_type)
        {
            return Ok(ItemsQueryResult {
                items: vec![],
                total_count: 0,
            });
        }

        // dbg!(&id, &media_type, "yoho");
        let mut item: jellyfin::BaseItemDto = state
            .stremio
            .get_meta(id.clone(), media_type.into(), None, None)
            .await
            .unwrap()
            .unwrap()
            .into();

        // jf is weird. if a stream id is provider it needs the be the id
        if stream_id.is_some() {
            item.id = uuid.to_string();
        };

        item.media_sources = Some(
            state
                .stremio
                .get_streams(
                    id.clone(),
                    media_type.into(),
                    None,
                    None, //  item.parent_index_number,
                          //  item.index_number,
                )
                .await
                .unwrap()
                .into_iter()
                // jf is weird. if a stream id is provider we just filter out the rest.
                .filter(|x| stream_id.clone().map_or(true, |id| x.id() == id))
                .map(|stream| {
                    let mut source = stream.into_media_source();
                    let _id = Some(utils::encode_media_uuid(
                        &id,
                        media_type,
                        Some(stream.id()),
                    ))
                    .unwrap();
                    source.id = Some(_id.clone());
                    source.e_tag = Some(_id);
                    source
                })
                .collect::<Vec<jellyfin::MediaSourceInfo>>(),
        );
        items.push(item);
    } else {
        // check parent if its a library
        let mut catalog = if let Some(parent_id) = &q.parent_id {
            if parent_id.contains("movie") || parent_id.contains("series") {
                sdks::stremio::MediaType::from_str(parent_id)
                    .ok()
                    .and_then(|ty| state.stremio.get_library_catalog(ty))
            } else {
                None
            }
        } else {
            None
        };

        if catalog.is_none() && q.include_item_types.is_some() {
            let media_type = q.include_item_types.unwrap()[0];
            if let Some(ref t) = search {
                catalog = state.stremio.get_search_catalog(media_type.into());
            } else {
                catalog = state.stremio.get_library_catalog(media_type.into());
            }
        } else {
            if catalog.is_none() {
                catalog = state.stremio.get_catalogs().first().cloned();
            };
        }

        let catalog = catalog.expect("at least one catalog should exist");
        //dbg!(&catalog);
        items = state
            .stremio
            .get_catalog_items(catalog.uuid.clone(), search, q.limit, Some(skip))
            .await?
            .into_iter()
            .map(jellyfin::BaseItemDto::from)
            .collect();
    }

    return Ok(ItemsQueryResult {
        items: items,
        total_count: 999999,
    });
}

pub async fn items_flat(
    State(state): State<AppState>,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let items = get_items(state, q, false).await?;
    Ok(Json::<Vec<jellyfin::BaseItemDto>>(items.items))
}

pub async fn items(
    State(state): State<AppState>,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let items = get_items(state, q.clone(), true).await?;

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i32,
        start_index: q.start_index.unwrap_or_else(|| 0),
        ..Default::default()
    }))
}

pub async fn item(
    state: AppState,
    id: String,
) -> Result<Option<jellyfin::BaseItemDto>> {
    if let Some(library) = utils::libraries().into_iter().find(|x| x.id.clone() == id) {
        return Ok(Some(library));
    }

    let q = jellyfin::GetItemsQuery {
        ids: vec![id].into(),
        ..Default::default()
    };
    return Ok(get_items(state, q, false).await?.items.first().cloned());
}

pub async fn items_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse> {
    return Ok(Json(item(state, id).await?).into_response());
}

pub async fn users_items_get(
    State(state): State<AppState>,
    Path((user_id, id)): Path<(String, String)>,
) -> Result<impl IntoResponse> {
    return Ok(Json(item(state, id).await?).into_response());
}

pub async fn shows_seasons(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(mut q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    q.parent_id = Some(id);
    q.include_item_types = Some(vec![jellyfin::MediaType::Season]);
    let items = get_items(state, q.clone(), true).await?;

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        ..Default::default()
    }))
}

pub async fn shows_episodes(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(mut q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    // q.season_id = Some(id);
    q.parent_id = Some(id);
    q.include_item_types = Some(vec![jellyfin::MediaType::Episode]);
    let items = get_items(state, q.clone(), true).await?;

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
            utils::decode_media_uuid(&id).log_err("Failed to decode media UUID")?;

        let ids: Vec<String> = id.split(':').map(|s| s.to_string()).collect();

        let mut base_media_type = media_type;
        if media_type == jellyfin::MediaType::Episode
            || media_type == jellyfin::MediaType::Season
        {
            base_media_type = jellyfin::MediaType::Series;
        }
        // dbg!(&ids, media_type, "items_images");

        // get details
        let meta = state
            .stremio
            .get_meta(ids[0].clone(), base_media_type.into(), None, None)
            .await?
            .ok_or_else(|| eyre::eyre!("missing meta"))?;
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
    Path(id): Path<String>,
    Query(query): Query<jellyfin::PlaybackInfoQuery>,
    Json(payload): Json<jellyfin::PlaybackInfoQuery>,
) -> Result<impl IntoResponse> {
    // let Json(payload) = data.unwrap_or_default();
    trace!(?payload, "items_playbackinfo");
    let (id, media_type, stream_id) = utils::decode_media_uuid(&id)
        .log_err("Failed to decode media UUID")
        .unwrap();

    trace!(?id, ?media_type, ?stream_id, "items_playbackinfo");

    /// todo: media source can also be send as streamid.
    let media_source_id: Option<String> = payload
        .media_source_id
        .clone()
        .or_else(|| query.media_source_id.clone())
        .and_then(|s| {
            let (_, _, stream_id) = utils::decode_media_uuid(&s)
                .log_err("Failed to decode media UUID")
                .unwrap();
            stream_id
        });

    let filter_by_id: Option<String> = stream_id.or(media_source_id.clone());
    // dbg!(&filter_by_id, "filter_by_id");
    let mut streams: Vec<sdks::stremio::Stream> = state
        .stremio
        .get_streams(id.clone(), media_type.into(), None, None)
        .await?
        .into_iter()
        .filter(|x| {
            // we dont need everything. Only the selected
            filter_by_id
                .clone()
                .map(|id| {
                    // dbg!(x.id(), "filtering");
                    let result = x.id() == *id;
                    result
                })
                .unwrap_or(true)
        })
        .collect();

    // fallback
    streams = vec![streams[0].clone()];
    // dbg!(&streams);

    // let subtitles = state
    //     .stremio
    //     .get_subtitles(id, media_type.into(), None, None)
    //     .await?;
    let subtitles: Vec<stremio::Subtitle> = vec![];

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
    Path(uuid): Path<String>,
    Query(q): Query<jellyfin::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    let (id, media_type, stream_id) = utils::decode_media_uuid(&uuid)
        .log_err("Failed to decode media UUID")
        .unwrap();

    trace!(?uuid, ?q, ?id, ?headers, ?stream_id, "videos_stream");

    let streams = state
        .stremio
        .get_streams(id, media_type.into(), None, None)
        .await
        .unwrap();

    // filter by id
    let filter_by_id: Option<String> = stream_id.or(q.media_source_id.clone());
    let stream = streams
        .into_iter()
        .find(|x| {
            filter_by_id
                .as_ref()
                .map(|id| x.id() == *id)
                .unwrap_or(true)
        })
        .unwrap();

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
