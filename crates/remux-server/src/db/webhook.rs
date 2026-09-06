use anyhow::Result;
use chrono::Utc;
use remux_sdks::remux::WebhookEvent;
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
    pub url: String,
    #[serde(default)]
    pub events: Vec<WebhookEvent>,
    #[serde(default)]
    pub user_ids: Vec<Uuid>,
    #[serde(default)]
    pub media_types: Vec<String>,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub fields: HashMap<String, String>,
    #[serde(default)]
    pub send_all_properties: bool,
    #[serde(default)]
    pub trim_whitespace: bool,
    #[serde(default)]
    pub skip_empty_body: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookDelivery {
    pub id: Uuid,
    pub webhook_id: Uuid,
    pub event: String,
    pub attempt: i64,
    pub success: bool,
    pub status_code: Option<i64>,
    pub error: Option<String>,
    pub created_at: String,
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
        let headers = serde_json::to_string(&self.headers)?;
        let fields = serde_json::to_string(&self.fields)?;
        sqlx::query("INSERT INTO webhooks (id,name,enabled,url,events,user_ids,media_types,template,headers,fields,send_all_properties,trim_whitespace,skip_empty_body,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,enabled=excluded.enabled,url=excluded.url,events=excluded.events,user_ids=excluded.user_ids,media_types=excluded.media_types,template=excluded.template,headers=excluded.headers,fields=excluded.fields,send_all_properties=excluded.send_all_properties,trim_whitespace=excluded.trim_whitespace,skip_empty_body=excluded.skip_empty_body,updated_at=excluded.updated_at")
            .bind(self.id).bind(&self.name).bind(self.enabled).bind(&self.url).bind(events).bind(users).bind(media).bind(&self.template).bind(headers).bind(fields).bind(self.send_all_properties).bind(self.trim_whitespace).bind(self.skip_empty_body).bind(&self.created_at).bind(&self.updated_at).execute(db).await?;
        Ok(())
    }

    pub async fn delete(db: &SqlitePool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM webhooks WHERE id = ?")
            .bind(id)
            .execute(db)
            .await?;
        Ok(())
    }

    pub async fn record_delivery(
        db: &SqlitePool,
        webhook_id: Uuid,
        event: &str,
        attempt: i64,
        success: bool,
        status_code: Option<u16>,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO webhook_deliveries (id,webhook_id,event,attempt,success,status_code,error,created_at) VALUES (?,?,?,?,?,?,?,?)")
            .bind(Uuid::new_v4()).bind(webhook_id).bind(event).bind(attempt).bind(success).bind(status_code.map(i64::from)).bind(error.map(|e| e.chars().take(500).collect::<String>())).bind(Utc::now().to_rfc3339()).execute(db).await?;
        Ok(())
    }

    pub async fn deliveries(
        db: &SqlitePool,
        id: Uuid,
        limit: i64,
    ) -> Result<Vec<WebhookDelivery>> {
        Ok(sqlx::query_as::<_, WebhookDeliveryRow>("SELECT * FROM webhook_deliveries WHERE webhook_id = ? ORDER BY created_at DESC LIMIT ?")
            .bind(id).bind(limit.clamp(1, 200)).fetch_all(db).await?.into_iter().map(Into::into).collect())
    }
}

#[derive(sqlx::FromRow)]
struct WebhookRow {
    id: Uuid,
    name: String,
    enabled: bool,
    url: String,
    events: String,
    user_ids: String,
    media_types: String,
    template: String,
    headers: String,
    fields: String,
    send_all_properties: bool,
    trim_whitespace: bool,
    skip_empty_body: bool,
    created_at: String,
    updated_at: String,
}
impl WebhookRow {
    fn into_config(self) -> Result<WebhookConfig> {
        Ok(WebhookConfig {
            id: self.id,
            name: self.name,
            enabled: self.enabled,
            url: self.url,
            events: serde_json::from_str(&self.events)?,
            user_ids: serde_json::from_str(&self.user_ids)?,
            media_types: serde_json::from_str(&self.media_types)?,
            template: self.template,
            headers: serde_json::from_str(&self.headers)?,
            fields: serde_json::from_str(&self.fields)?,
            send_all_properties: self.send_all_properties,
            trim_whitespace: self.trim_whitespace,
            skip_empty_body: self.skip_empty_body,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}
#[derive(sqlx::FromRow)]
struct WebhookDeliveryRow {
    id: Uuid,
    webhook_id: Uuid,
    event: String,
    attempt: i64,
    success: bool,
    status_code: Option<i64>,
    error: Option<String>,
    created_at: String,
}
impl From<WebhookDeliveryRow> for WebhookDelivery {
    fn from(r: WebhookDeliveryRow) -> Self {
        Self {
            id: r.id,
            webhook_id: r.webhook_id,
            event: r.event,
            attempt: r.attempt,
            success: r.success,
            status_code: r.status_code,
            error: r.error,
            created_at: r.created_at,
        }
    }
}
