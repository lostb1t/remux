use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Media::Table)
                    .if_not_exists()
                    // .col(pk_auto(Media::Id))
                    // .col(string(Media::Id))
                    .col(string(Media::Id).not_null().primary_key().unique_key())
                    .col(string(Media::Name))
                    .col(integer_null(Media::TmdbId))
                    .col(integer_null(Media::ParentId))
                    // .col(string_null(Media::ImdbId))
                    .col(string_null(Media::Overview))
                    .col(float_null(Media::Rating))
                    .col(integer_null(Media::Runtime))
                    .col(date_null(Media::ReleaseDate))
                    .col(string_null(Media::PosterPath))
                    .col(string_null(Media::BackdropPath))
                    .col(string(Media::MediaType))
                    .col(integer_null(Media::Status))
                    .col(integer_null(Media::IndexNumber))
                    .col(integer_null(Media::ParentIndexNumber))
                    .col(float_null(Media::CommunityRating))
                    .col(float_null(Media::CriticRating))
                    // .index(
                    //     Index::create()
                    //         .name("idx-tmdb-mediatype")
                    //         .table(Media::Table)
                    //         .col(Media::TmdbId)
                    //         .col(Media::MediaType) // Add this line for composite index
                    //         .unique(),
                    // )
                    .to_owned(),
            )
            .await?;
        // manager
        //     .create_index(
        //         Index::create()
        //             .name("idx-mediatype")
        //             .table(Media::Table)
        //             .col(Media::MediaType)
        //             .to_owned(),
        //     )
        //     .await?;
        // manager
        //    .create_table(
        //        Table::create()
        //           .table(Genre::Table)
        //           .if_not_exists()
        //          .col(pk_auto(Genre::Id))
        //          .col(string(Genre::Name))
        //          .index(
        //             Index::create()
        //                .name("idx-name")
        //                .table(Genre::Table)
        //                 .col(Genre::Name)
        //                .unique(),
        //        )
        //        .to_owned(),
        //)
        //.await?;
        manager
            .create_table(
                Table::create()
                    .table(MediaGenre::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(MediaGenre::MediaId).integer().not_null())
                    .col(string(MediaGenre::Genre))
                    // .col(ColumnDef::new(MediaGenre::GenreId).integer().not_null())
                    .primary_key(
                        Index::create()
                            .col(MediaGenre::MediaId)
                            .col(MediaGenre::Genre),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(MediaGenre::Table, MediaGenre::MediaId)
                            .to(Media::Table, Media::Id),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Media::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Media {
    Table,
    Id,
    // ImdbId,
    TmdbId,
    ParentId,
    Name,
    Overview,
    Rating,
    Runtime,
    ReleaseDate,
    PosterPath,
    BackdropPath,
    MediaType,
    Status,
    IndexNumber,
    ParentIndexNumber,
    CommunityRating,
    CriticRating,
}

#[derive(Iden)]
enum Genre {
    Table,
    Id,
    Name,
}

#[derive(Iden)]
enum MediaGenre {
    Table,
    MediaId,
    Genre,
}
