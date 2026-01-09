use anyhow::Result;
use diesel::SqliteConnection;
use diesel::r2d2::ConnectionManager;
use r2d2::Pool;
use std::sync::Arc;

pub mod auth;
pub mod media;
pub mod user;
pub use media::*;
pub use user::*;
pub mod schema;

type DbPool = Pool<ConnectionManager<SqliteConnection>>;

#[derive(Clone)]
pub struct DbConn {
    pool: Arc<DbPool>,
}

impl DbConn {
    pub fn new(database_url: &str) -> Result<Self> {
        let manager = ConnectionManager::<SqliteConnection>::new(database_url);
        let pool = Pool::builder().max_size(10).build(manager)?;
        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    pub fn get_conn(
        &self,
    ) -> Result<r2d2::PooledConnection<ConnectionManager<SqliteConnection>>> {
        Ok(self.pool.clone().get()?)
    }
}

pub fn migrate(db: &DbConn) -> Result<()> {
    let mut conn = db.get_conn()?;
    diesel_migrations::run_pending_migrations(&mut conn)?;
    Ok(())
}
