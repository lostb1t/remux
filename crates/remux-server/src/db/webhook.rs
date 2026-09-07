use anyhow::Result;
use remux_sdks::remux::{WebhookDestination, WebhookEvent};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebhookConfig {
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

impl WebhookConfig {
    pub async fn list(db: &SqlitePool) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, WebhookRow>(
            "SELECT * FROM webhooks ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(db)
        .await?;
        rows.into_iter()
            .map(WebhookRow::into_config)
            .collect()
    }

    pub async fn get(db: &SqlitePool, id: Uuid) -> Result<Option<Self>> {
        Ok(
            sqlx::query_as::<_, WebhookRow>("SELECT * FROM webhooks WHERE id = ?")
                .bind(id)
                .fetch_optional(db)
                .await?
                .map(WebhookRow::into_config)
                .transpose()?,
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

#[derive(sqlx::FromRow)]
struct WebhookRow {
    id: Uuid,
    name: String,
    enabled: bool,
    destination: String,
    events: String,
    user_ids: String,
    media_types: String,
    template: String,
    fields: String,
    send_all_properties: bool,
    trim_whitespace: bool,
    skip_empty_body: bool,
}
impl WebhookRow {
    fn into_config(self) -> Result<WebhookConfig> {
        Ok(WebhookConfig {
            id: self.id,
            name: self.name,
            enabled: self.enabled,
            destination: serde_json::from_str(&self.destination)?,
            events: serde_json::from_str(&self.events)?,
            user_ids: serde_json::from_str(&self.user_ids)?,
            media_types: serde_json::from_str(&self.media_types)?,
            template: self.template,
            fields: serde_json::from_str(&self.fields)?,
            send_all_properties: self.send_all_properties,
            trim_whitespace: self.trim_whitespace,
            skip_empty_body: self.skip_empty_body,
        })
    }
}
