use super::Genre;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "media_genre")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub media_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    //pub genre_id: i64,
    pub genre: Genre,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::media::Entity",
        from = "Column::MediaId",
        to = "super::media::Column::Id"
    )]
    Media,
}

impl Related<super::media::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Media.def()
    }
}

//impl Related<super::genre::Entity> for Entity {
//    fn to() -> RelationDef {
//        Relation::Genre.def()
//    }
//}

impl ActiveModelBehavior for ActiveModel {}
