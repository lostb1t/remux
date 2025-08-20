use eyre::Result;
//use async_stream::stream;
use futures::StreamExt;
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, EntityTrait};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sea_orm::ColumnTrait;
use sea_orm::QueryFilter;
use crate::db;
use crate::sdks::tmdb::{Endpoint, IdSetter, TmdbClient};
use crate::utils;


#[derive(Debug, Deserialize, Clone)]
struct TmdbId {
    id: i64,
}

// for shows. Rhey have an active state or not. So we can scrape only active shows for new content
//.we only need stills for episodes.
// so 2 calls. Shows with seasons. And rhen every season for episodes and crews.
// so yeah need soecial handmer for it... duck

// people: has id export

// NEED TO ADD CAST/Credits loading aswell.
// Its in a derisl page of movie and episode

// fuck networks

// LOAD SEASONS AND EPISODES REALTIME FROM TMDB
// saving space

// not sure about actors. could probaply also lisd them realtime.
// guess only genres left

pub async fn import_tmdb<E>(
    url: &str,
    media_type: db::media::MediaType,
    conn: &db::Database,
    tmdb_client: &TmdbClient,
) -> Result<()>
where
    E: Endpoint + Send + Sync + Clone + 'static,
    E::Output: DeserializeOwned + Send + Sync,
    db::media::Model: From<E::Output>,
{
    let start_id: i64 = db::media::Model::get_latest_by_media_type(conn, media_type)
        .await
        .map(|inner| inner.tmdb_id.clone().unwrap_or(0))
        .unwrap_or(0);

    tracing::info!(
        "Starting tmdb import from {}, starting at id {}",
        url,
        start_id
    );

    let stream = utils::FileStream::<TmdbId>::from_url(url).await?;
    let chunk_size = 60;
    let mut imported = 0;
    let mut chunks = stream.chunks(chunk_size);

    while let Some(chunk) = chunks.next().await {
        let ids = chunk
            .into_iter()
            .map(Result::unwrap)
            .map(|x| x.id)
            .filter(|&id| id >= start_id)
            .collect::<Vec<i64>>();

        if ids.is_empty() {
            continue;
        }

        //  dbg!(&ids);

        import_many::<E>(&ids, conn, tmdb_client).await?;
        imported += ids.len();

        tracing::info!("Imported {} items so far", imported);
    }
    tracing::info!("finished");
    Ok(())
}

pub async fn import_many<E>(
    ids: &[i64],
    conn: &db::Database,
    tmdb_client: &TmdbClient,
) -> Result<()>
where
    E: Endpoint + Send + Sync + Clone + 'static,
    //E::Builder: Clone + Default,
    //E::Builder: Clone + Default + bon::Build<Output = E> + IdSetter,
    E::Output: DeserializeOwned + Send + Sync,
    db::media::Model: From<E::Output>,
{
    let futures = ids.iter().map(|&id| {
        let conn = conn.clone();
        let client = tmdb_client.clone();
        let endpoint = E::build(id);
        async move { import_one::<E>(endpoint, conn, &client).await }
    });

    futures::future::try_join_all(futures).await?;
    Ok(())
}

pub async fn import_one<E>(endpoint: E, conn: db::Database, tmdb_client: &TmdbClient) -> Result<()>
where
    E: Endpoint + Send + Sync,
    E::Output: DeserializeOwned + Send + Sync,
    db::media::Model: From<E::Output>,
{
    let res = tmdb_client.request(&endpoint).await;
    let item = match res {
        Ok(ok) => ok,
        Err(err) => {
            tracing::warn!("Loading failed, skipping: {:?}", err);
            return Ok(());
        }
    };
    
    let stateless_media = db::media::Model::from(item);
    let mut active_model: db::media::ActiveModel = stateless_media.clone().into();
    active_model.id = NotSet;

    let result = db::media::Entity::insert(active_model)
        .on_conflict(
            sea_orm::sea_query::OnConflict::columns([
                db::media::Column::TmdbId,
                db::media::Column::MediaType,
            ])
            .update_column(db::media::Column::Name)
            .update_column(db::media::Column::MediaType)
            .to_owned(),
        )
        .exec(&conn.pool)
        .await?;

    // get our new media
    let media = db::media::Model::get_by_tmdb(
        &conn,
        stateless_media.tmdb_id.unwrap() as u64,
        stateless_media.media_type,
    )
    .await
    .unwrap();
    
    db::media_genre::Entity::delete_many()
    .filter(db::media_genre::Column::MediaId.eq(media.id))
    .exec(&conn.pool)
    .await?;
    
    if let Some(tmdb_genres) = stateless_media.genres {
    let genre_models: Vec<db::media_genre::ActiveModel> = tmdb_genres
        .into_iter()
        .filter_map(|tmdb_genre| {
            Some(db::media_genre::ActiveModel {
                    media_id: sea_orm::Set(media.id),
                    genre: sea_orm::Set(tmdb_genre),
                })
        })
        .collect();

   // if !genre_models.is_empty() {
   //     let _ = db::media_genre::Entity::insert_many(genre_models)
   //         .exec(&conn.pool)
   //         .await?;
   // }
    }
    // Load children. Only applicanle for series
    media
        .refresh_children(&conn, &tmdb_client, None)
        .await
        .unwrap_or_else(|err| {
            tracing::error!("Failed: {:?}", err);
        });

    Ok(())
}
