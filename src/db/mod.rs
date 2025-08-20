//pub mod item;
//pub use item::*;

pub mod genre;
pub mod media;
pub mod media_genre;
pub mod migrations;
pub use genre::Genre;

use eyre::Result;
use migrations::{Migrator, MigratorTrait};
use sea_orm;
pub use sea_orm::DatabaseConnection as DbConnection;
use sea_orm::Statement;
use sea_orm::sqlx;
pub use sea_orm::{ConnectOptions, DatabaseConnection};
use sea_orm::{
    DatabaseBackend, IntoActiveModel, Set, TryIntoModel, entity::prelude::*,
};

#[derive(Debug, Clone)]
pub struct Database {
    pub pool: DatabaseConnection,
}

impl Database {
    pub async fn new() -> Result<Self> {
        let url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://db.sqlite?mode=rwc".to_string());
        let mut opt = ConnectOptions::new(url.to_string()); // mutable because we're configuring it
        opt.max_connections(5);

        let pool = sea_orm::Database::connect(opt).await?;
        pool.execute(Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            "PRAGMA journal_mode = WAL;".to_owned(),
        ))
        .await?;
        Migrator::up(&pool, None)
            .await
            .expect("Failed to run migrations");
        //Ok(db)

        Ok(Self { pool })
    }
}
