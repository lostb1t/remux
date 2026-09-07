use anyhow::Result;
use remux_sdks::remux::{WebhookDestination, WebhookEvent};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Webhook {
    pub id: Uuid,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub destination: WebhookDestination,
    #[serde(default)]
    pub events: Vec<WebhookEvent>,
    #[serde(default)]
    pub user_ids: Vec<Uuid>,
    #[serde(default)]
    pub media_types: Vec<String>,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub fields: HashMap<String, String>,
    #[serde(default)]
    pub send_all_properties: bool,
    #[serde(default)]
    pub trim_whitespace: bool,
    #[serde(default)]
    pub skip_empty_body: bool,
}

fn default_true() -> bool {
    true
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Webhook {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
        let destination: String = row.try_get("destination")?;
        let events: String = row.try_get("events")?;
        let user_ids: String = row.try_get("user_ids")?;
        let media_types: String = row.try_get("media_types")?;
        let fields: String = row.try_get("fields")?;
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            enabled: row.try_get("enabled")?,
            destination: serde_json::from_str(&destination)
                .map_err(|e| sqlx::Error::Decode(e.into()))?,
            events: serde_json::from_str(&events)
                .map_err(|e| sqlx::Error::Decode(e.into()))?,
            user_ids: serde_json::from_str(&user_ids)
                .map_err(|e| sqlx::Error::Decode(e.into()))?,
            media_types: serde_json::from_str(&media_types)
                .map_err(|e| sqlx::Error::Decode(e.into()))?,
            template: row.try_get("template")?,
            fields: serde_json::from_str(&fields)
                .map_err(|e| sqlx::Error::Decode(e.into()))?,
            send_all_properties: row.try_get("send_all_properties")?,
            trim_whitespace: row.try_get("trim_whitespace")?,
            skip_empty_body: row.try_get("skip_empty_body")?,
        })
    }
}

impl Webhook {
    pub async fn list(db: &SqlitePool) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(
            "SELECT * FROM webhooks ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(db)
        .await?)
    }

    pub async fn get(db: &SqlitePool, id: Uuid) -> Result<Option<Self>> {
        Ok(
            sqlx::query_as::<_, Self>("SELECT * FROM webhooks WHERE id = ?")
                .bind(id)
                .fetch_optional(db)
                .await?,
        )
    }

    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        let events = serde_json::to_string(&self.events)?;
        let users = serde_json::to_string(&self.user_ids)?;
        let media = serde_json::to_string(&self.media_types)?;
        let destination = serde_json::to_string(&self.destination)?;
        let fields = serde_json::to_string(&self.fields)?;
        sqlx::query("INSERT INTO webhooks (id,name,enabled,destination,events,user_ids,media_types,template,fields,send_all_properties,trim_whitespace,skip_empty_body) VALUES (?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,enabled=excluded.enabled,destination=excluded.destination,events=excluded.events,user_ids=excluded.user_ids,media_types=excluded.media_types,template=excluded.template,fields=excluded.fields,send_all_properties=excluded.send_all_properties,trim_whitespace=excluded.trim_whitespace,skip_empty_body=excluded.skip_empty_body")
            .bind(self.id).bind(&self.name).bind(self.enabled).bind(destination).bind(events).bind(users).bind(media).bind(&self.template).bind(fields).bind(self.send_all_properties).bind(self.trim_whitespace).bind(self.skip_empty_body).execute(db).await?;
        Ok(())
    }

    pub async fn delete(db: &SqlitePool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM webhooks WHERE id = ?")
            .bind(id)
            .execute(db)
            .await?;
        Ok(())
    }
}
