use crate::sdks;
use chrono::NaiveDate;
use eyre::{Result, eyre};
use sdks::tmdb;
use sea_orm::QueryOrder;
use sea_orm::{
    ActiveValue::{self, NotSet},
    IntoActiveModel, Set, TryIntoModel,
    entity::prelude::*,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::From;
use strum_macros;

#[derive(
    Copy,
    Serialize,
    Debug,
    Clone,
    Eq,
    PartialEq,
    Deserialize,
    Hash,
    EnumIter,
    DeriveActiveEnum,
    strum_macros::Display,
    // strum_macros::EnumString,
    Default,
)]
#[serde(rename_all = "PascalCase")]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "PascalCase"
)]
pub enum MediaType {
    AggregateFolder,
    Audio,
    AudioBook,
    BasePluginFolder,
    Book,
    BoxSet,
    Channel,
    ChannelFolderItem,
    CollectionFolder,
    Episode,
    Folder,
    Genre,
    ManualPlaylistsFolder,
    Movie,
    LiveTvChannel,
    LiveTvProgram,
    MusicAlbum,
    MusicArtist,
    MusicGenre,
    MusicVideo,
    Person,
    Photo,
    PhotoAlbum,
    Playlist,
    PlaylistsFolder,
    Program,
    Recording,
    Season,
    Series,
    Studio,
    Trailer,
    TvChannel,
    TvProgram,
    UserRootFolder,
    UserView,
    Video,
    Year,
    #[default]
    Unknown,
}

#[derive(
    Copy,
    Serialize,
    Debug,
    Clone,
    Eq,
    PartialEq,
    Deserialize,
    Hash,
    EnumIter,
    DeriveActiveEnum,
    strum_macros::Display,
    //   strum_macros::EnumString,
)]
#[serde(rename_all = "lowercase")]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "lowercase"
)]
pub enum Genre {
    Action,
}

#[derive(
    Copy,
    Serialize,
    Debug,
    Clone,
    Eq,
    PartialEq,
    Deserialize,
    Hash,
    EnumIter,
    DeriveActiveEnum,
    strum_macros::Display,
    strum_macros::EnumString,
)]
#[serde(rename_all = "PascalCase")]
#[sea_orm(rs_type = "i32", db_type = "Integer")]
pub enum Status {
    #[serde(rename = "Rumored")]
    Rumored = 1,

    #[serde(rename = "Planned")]
    Planned = 2,

    #[serde(rename = "In Production")]
    InProduction = 3,

    #[serde(rename = "Post Production")]
    PostProduction = 4,

    #[serde(rename = "Released")]
    Released = 5,

    #[serde(rename = "Canceled")]
    Canceled = 6,

    #[serde(rename = "Pilot")]
    Pilot = 7,

    #[serde(rename = "Returning Series")]
    ReturningSeries = 8,

    #[serde(rename = "Ended")]
    Ended = 9,
}

#[derive(Clone, Debug, Default, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "media")]
//#[schema(as = Media)]
pub struct Model {
    #[serde(skip_deserializing)]
    #[sea_orm(primary_key)]
    pub id: String, // imdb id, cause fuckit
    pub tmdb_id: Option<i64>,
    // pub imdb_id: Option<String>,
    pub parent_id: Option<i64>,
    pub name: String,
    pub overview: Option<String>,
    /// in minutes
    pub runtime: Option<i64>,
    /// rating provided by any API that is encoded as a signed integer. Usually TMDB rating.
    pub rating: Option<f64>,
    pub release_date: Option<NaiveDate>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub media_type: MediaType,
    pub status: Option<Status>,
    //pub genres: Option<Vec<Genre>>,
    pub index_number: Option<i64>,
    pub parent_index_number: Option<i64>,
    pub community_rating: Option<f64>,
    pub critic_rating: Option<f64>,

    // normalization
    // pub series_id: Option<i64>,
    // pub season_id: Option<i64>,
    //pub season: Option<i64>,
    #[sea_orm(ignore)]
    pub streams: Option<Vec<sdks::stremio::Stream>>,
    //#[sea_orm(ignore)]
    // pub resources: Option<sdks::stremio::Resources>,
    #[sea_orm(ignore)]
    pub genres: Option<Vec<super::Genre>>,
}

impl Model {
    //pub fn get_imdb_id(&self) -> Option<String>
    //match self.media_type {
    //MediaType:: Episode => self.get_parent().unwrap()c

    //}
    //}
    // pub async fn get_parent(&self, db: &super::Database) -> Result<Option<Self>> {
    //     Ok(Entity::find_by_id(self.parent_id.clone().unwrap())
    //         .one(&db.pool)
    //         .await?)
    // }

    pub async fn get_latest_by_media_type(
        db: &super::Database,
        media_type_value: MediaType,
    ) -> Option<Self> {
        Entity::find()
            .filter(Column::MediaType.eq(media_type_value))
            .order_by(Column::Id, sea_orm::Order::Desc)
            .one(&db.pool)
            .await
            .ok()?
    }

    pub async fn get_by_tmdb(
        db: &super::Database,
        tmdb_id: u64,
        media_type_value: MediaType,
    ) -> Option<Self> {
        Entity::find()
            .filter(Column::MediaType.eq(media_type_value))
            .filter(Column::TmdbId.eq(tmdb_id))
            //  .order_by(Column::Id, sea_orm::Order::Desc)
            .one(&db.pool)
            .await
            .ok()?
    }

    pub async fn create(self, db: &DbConn) -> Result<Self> {
        let mut active_model: ActiveModel = self.try_into()?;
        active_model.id = NotSet;
        Ok(active_model.insert(db).await?.try_into()?)
    }

    pub async fn bulk_upsert(items: Vec<Self>, db: &super::Database) -> Result<()> {
        let active_models: Vec<ActiveModel> = items
            .into_iter()
            .map(|model| {
                let mut am = ActiveModel::from(model);
                //  am.id = NotSet;
                am
            })
            .collect();

        Entity::insert_many(active_models)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(Column::Id)
                    //   .update_column(Column::Name)
                    // .update_column(Column::MediaType)
                    .do_nothing()
                    .to_owned(),
            )
            .exec(&db.pool)
            .await?;

        Ok(())
    }

    // pub async fn refresh_children(
    //     &self,
    //     db: &super::Database,
    //     tmdb_client: &tmdb::TmdbClient,
    //     parent: Option<Model>,
    // ) -> Result<()> {
    //     let items: Vec<Model> = match self.media_type {
    //         MediaType::Series => {
    //             let endpoint = tmdb::SeriesEndpoint::builder()
    //                 .id(self.tmdb_id.clone().unwrap())
    //                 .build();
    //             //let series = tmdb_client.request(&endpoint).await?;
    //             let series = endpoint.request(&tmdb_client).await?;
    //             series
    //                 .seasons
    //                 .into_iter()
    //                 .map(Model::from)
    //                 .map(|mut item| {
    //                     item.parent_id = Some(self.id.clone());
    //                     item.imdb_id = self.imdb_id.clone();
    //                     item
    //                 })
    //                 .collect()
    //         }
    //         MediaType::Season => {
    //             let endpoint = tmdb::SeasonEndpoint::builder()
    //                 .season_number(self.index_number.unwrap())
    //                 .series_id(parent.as_ref().unwrap().tmdb_id.clone().unwrap())
    //                 .build();
    //             let season = endpoint.request(&tmdb_client).await?;
    //             season
    //                 .episodes
    //                 .unwrap_or_default()
    //                 .into_iter()
    //                 .map(Model::from)
    //                 .map(|mut item| {
    //                     item.parent_id = Some(self.id.clone());
    //                     item.imdb_id = self.imdb_id.clone();
    //                     item
    //                 })
    //                 .collect()
    //         }
    //         _ => vec![],
    //     };

    //     if items.is_empty() {
    //         return Ok(());
    //     }

    //     // Prepare and insert models
    //     let active_models: Vec<ActiveModel> = items
    //         .into_iter()
    //         .map(|model| {
    //             let mut am = ActiveModel::from(model);
    //             am.id = NotSet;
    //             am
    //         })
    //         .collect();

    //     Entity::insert_many(active_models)
    //         .on_conflict(
    //             sea_orm::sea_query::OnConflict::columns([Column::TmdbId, Column::MediaType])
    //                 .update_column(Column::Name)
    //                 .update_column(Column::MediaType)
    //                 .to_owned(),
    //         )
    //         .exec(&db.pool)
    //         .await?;

    //     // Recurse into newly inserted items (only for Seasons)
    //     if self.media_type == MediaType::Series {
    //         let inserted_media: Vec<Model> = Entity::find()
    //             .filter(Column::MediaType.eq(MediaType::Season))
    //             .filter(Column::ParentId.eq(self.id.clone()))
    //             .all(&db.pool)
    //             .await?;

    //         for m in inserted_media {
    //             Box::pin(m.refresh_children(&db, &tmdb_client, Some(self.clone()))).await?;
    //         }
    //     }

    //     Ok(())
    // }

    // pub fn get_ratings(&self) -> {

    // }

    //pub async fn resources(&self, stremio: &sdks::stremio::StremioClient) -> Result<sdks::stremio::Resources> {
    //                    Ok(stremio
    //                     .get_resources_flatten(
    //                        &self.imdb_id,
    //                         &self.media_type.into(),
    //                         None,
    //                        None,
    //                    )
    //                    .await?)

    //            }
}

impl PartialEq for Model {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::media_genre::Entity")]
    MediaGenre,
    //  #[sea_orm(has_many = "super::genre::Entity")]
    //  Genre,
}

// Assuming you have this relation in db::media
impl Related<super::media_genre::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MediaGenre.def()
    }
}

//impl Related<super::genre::Entity> for Entity {
// The final relation is Cake -> CakeFilling -> Filling
//fn to() -> RelationDef {
//    super::media_genre::Relation::Genre.def()
// }

//    fn via() -> Option<RelationDef> {
// The original relation is CakeFilling -> Cake,
// after `rev` it becomes Cake -> CakeFilling
//        Some(super::media_genre::Relation::Media.def().rev())
//    }
//}

impl ActiveModelBehavior for ActiveModel {}
