use anyhow::Result;
use remux_sdks::remux::{WebhookDestination, WebhookEvent};
use serde::{Deserialize, Serialize};
use sqlx::{SqlitePool, types::Json};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Webhook {
    pub id: Uuid,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub destination: Json<WebhookDestination>,
    #[serde(default)]
    pub events: Json<Vec<WebhookEvent>>,
    #[serde(default)]
    pub user_ids: Json<Vec<Uuid>>,
    #[serde(default)]
    pub media_types: Json<Vec<String>>,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub fields: Json<HashMap<String, String>>,
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
        sqlx::query("INSERT INTO webhooks (id,name,enabled,destination,events,user_ids,media_types,template,fields,send_all_properties,trim_whitespace,skip_empty_body) VALUES (?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,enabled=excluded.enabled,destination=excluded.destination,events=excluded.events,user_ids=excluded.user_ids,media_types=excluded.media_types,template=excluded.template,fields=excluded.fields,send_all_properties=excluded.send_all_properties,trim_whitespace=excluded.trim_whitespace,skip_empty_body=excluded.skip_empty_body")
            .bind(self.id).bind(&self.name).bind(self.enabled).bind(&self.destination).bind(&self.events).bind(&self.user_ids).bind(&self.media_types).bind(&self.template).bind(&self.fields).bind(self.send_all_properties).bind(self.trim_whitespace).bind(self.skip_empty_body).execute(db).await?;
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
