use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::common::get_uuid;

#[derive(
    Default,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type,
    strum_macros::Display,
    strum_macros::EnumString,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum IptvSourceType {
    #[default]
    M3u,
    Xtream,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct IptvSource {
    #[default(get_uuid())]
    pub id: Uuid,
    pub name: String,
    /// For M3U sources: the playlist URL.
    /// For Xtream sources: the server base URL (e.g. `http://host:port`).
    pub m3u_url: String,
    /// Deprecated — kept for schema compatibility; ignored in favour of EpgSource.
    pub epg_url: Option<String>,
    pub refresh_interval: String,
    pub source_type: IptvSourceType,
    pub xtream_username: Option<String>,
    pub xtream_password: Option<String>,
}

impl IptvSource {
    /// Return the XMLTV EPG URL for Xtream sources (auto-derived from credentials).
    /// Returns `None` for M3U sources (EPG is managed via separate `EpgSource` entries).
    pub fn xtream_epg_url(&self) -> Option<String> {
        if self.source_type != IptvSourceType::Xtream {
            return None;
        }
        let base = self
            .m3u_url
            .trim_end_matches('/');
        let user = self
            .xtream_username
            .as_deref()
            .unwrap_or("");
        let pass = self
            .xtream_password
            .as_deref()
            .unwrap_or("");
        Some(format!(
            "{}/xmltv.php?username={}&password={}",
            base, user, pass
        ))
    }

    /// For M3U sources, returns the playlist URL. For Xtream sources, returns the server base URL
    /// (channels are fetched via the native player API, not via this URL).
    pub fn m3u_playlist_url(&self) -> Option<String> {
        if self.source_type == IptvSourceType::M3u {
            Some(
                self.m3u_url
                    .clone(),
            )
        } else {
            None
        }
    }

    pub async fn get_all(db: &SqlitePool) -> Result<Vec<Self>> {
        Ok(
            sqlx::query_as::<_, Self>("SELECT * FROM iptv_sources ORDER BY name")
                .fetch_all(db)
                .await?,
        )
    }

    pub async fn get_by_id(db: &SqlitePool, id: &Uuid) -> Result<Option<Self>> {
        Ok(
            sqlx::query_as::<_, Self>("SELECT * FROM iptv_sources WHERE id = $1")
                .bind(id)
                .fetch_optional(db)
                .await?,
        )
    }

    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO iptv_sources (id, name, m3u_url, refresh_interval, source_type, xtream_username, xtream_password)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (id) DO UPDATE SET
                name               = excluded.name,
                m3u_url            = excluded.m3u_url,
                refresh_interval   = excluded.refresh_interval,
                source_type        = excluded.source_type,
                xtream_username    = excluded.xtream_username,
                xtream_password    = excluded.xtream_password
            "#,
        )
        .bind(self.id)
        .bind(&self.name)
        .bind(&self.m3u_url)
        .bind(&self.refresh_interval)
        .bind(&self.source_type)
        .bind(&self.xtream_username)
        .bind(&self.xtream_password)
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn delete(db: &SqlitePool, id: &Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM iptv_sources WHERE id = $1")
            .bind(id)
            .execute(db)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct EpgSource {
    #[default(get_uuid())]
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub refresh_interval: String,
}

impl EpgSource {
    pub async fn get_all(db: &SqlitePool) -> Result<Vec<Self>> {
        Ok(
            sqlx::query_as::<_, Self>("SELECT * FROM epg_sources ORDER BY name")
                .fetch_all(db)
                .await?,
        )
    }

    pub async fn get_by_id(db: &SqlitePool, id: &Uuid) -> Result<Option<Self>> {
        Ok(
            sqlx::query_as::<_, Self>("SELECT * FROM epg_sources WHERE id = $1")
                .bind(id)
                .fetch_optional(db)
                .await?,
        )
    }

    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO epg_sources (id, name, url, refresh_interval)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (id) DO UPDATE SET
                name             = excluded.name,
                url              = excluded.url,
                refresh_interval = excluded.refresh_interval
            "#,
        )
        .bind(self.id)
        .bind(&self.name)
        .bind(&self.url)
        .bind(&self.refresh_interval)
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn delete(db: &SqlitePool, id: &Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM epg_sources WHERE id = $1")
            .bind(id)
            .execute(db)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
