use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use chrono::{Datelike, Local, Utc};
use handlebars::{
    Context, Handlebars, Helper, HelperDef, HelperResult, JsonRender, Output,
    RenderContext, Renderable,
};
use http::{
    StatusCode,
    header::{HeaderName, HeaderValue},
};
use remux_macros::{delete, get, post, put};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    AppContext, AppState, OptionExt,
    db::{self, Webhook, auth::AdminSession},
    signals::{Event, EventType, PlaybackContext, Subscriber},
};
use async_trait::async_trait;
use axum_anyhow::ApiResult as Result;
use remux_sdks::remux::{HttpWebhookConfig, WebhookDestination, WebhookEvent};

static WEBHOOK_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build webhook client")
    });

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveWebhook {
    pub name: String,
    pub enabled: Option<bool>,
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

fn config_from(id: Uuid, p: SaveWebhook) -> Webhook {
    Webhook {
        id,
        name: p.name,
        enabled: p
            .enabled
            .unwrap_or(true),
        destination: sqlx::types::Json(p.destination),
        events: sqlx::types::Json(p.events),
        user_ids: sqlx::types::Json(p.user_ids),
        media_types: sqlx::types::Json(p.media_types),
        template: p.template,
        fields: sqlx::types::Json(p.fields),
        send_all_properties: p.send_all_properties,
        trim_whitespace: p.trim_whitespace,
        skip_empty_body: p.skip_empty_body,
    }
}

#[get("/remux/webhooks")]
pub async fn list(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<Vec<Webhook>>> {
    Ok(Json(
        Webhook::list(
            &state
                .ctx
                .db,
        )
        .await?,
    ))
}

#[post("/remux/webhooks")]
pub async fn create(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(p): Json<SaveWebhook>,
) -> Result<impl IntoResponse> {
    validate_destination(&p.destination)?;
    let config = config_from(Uuid::new_v4(), p);
    config
        .save(
            &state
                .ctx
                .db,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(config)))
}

#[put("/remux/webhooks/{id}")]
pub async fn update(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<Uuid>,
    Json(p): Json<SaveWebhook>,
) -> Result<Json<Webhook>> {
    validate_destination(&p.destination)?;
    Webhook::get(
        &state
            .ctx
            .db,
        id,
    )
    .await?
    .context_not_found("Webhook not found")?;
    let config = config_from(id, p);
    config
        .save(
            &state
                .ctx
                .db,
        )
        .await?;
    Ok(Json(config))
}

#[delete("/remux/webhooks/{id}")]
pub async fn delete(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    Webhook::delete(
        &state
            .ctx
            .db,
        id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRequest {
    pub template: String,
    #[serde(default)]
    pub send_all_properties: bool,
    #[serde(default)]
    pub trim_whitespace: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResponse {
    pub body: String,
}

#[post("/remux/webhooks/preview")]
pub async fn preview(
    State(_state): State<AppState>,
    _session: AdminSession,
    Json(p): Json<PreviewRequest>,
) -> Result<Json<PreviewResponse>> {
    let body = render_body(
        &p.template,
        p.send_all_properties,
        p.trim_whitespace,
        &sample_context(WebhookEvent::PlaybackStart),
    )?;
    Ok(Json(PreviewResponse { body }))
}

#[post("/remux/webhooks/{id}/test")]
pub async fn test(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<Uuid>,
) -> Result<Json<PreviewResponse>> {
    let config = Webhook::get(
        &state
            .ctx
            .db,
        id,
    )
    .await?
    .context_not_found("Webhook not found")?;
    let event = config
        .events
        .first()
        .copied()
        .unwrap_or(WebhookEvent::PlaybackStart);
    let body = send_webhook(&config, event, sample_context(event)).await?;
    Ok(Json(PreviewResponse { body }))
}

fn validate_destination(destination: &WebhookDestination) -> anyhow::Result<()> {
    match destination {
        WebhookDestination::Http(config) => {
            validate_url(&config.url)?;
            for (key, value) in &config.headers {
                HeaderName::try_from(key)
                    .map_err(|_| anyhow::anyhow!("invalid header name: {key}"))?;
                HeaderValue::try_from(value)
                    .map_err(|_| anyhow::anyhow!("invalid header value for {key}"))?;
            }
            Ok(())
        }
    }
}

fn validate_url(raw: &str) -> anyhow::Result<()> {
    let url = url::Url::parse(raw)?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https"),
        "webhook URL must use http or https"
    );
    Ok(())
}

fn sample_context(event: WebhookEvent) -> Value {
    json!({ "NotificationType": event.to_string(), "ServerName": "Remux", "ServerUrl": "", "Name": "Example Item", "ItemType": "Movie", "ItemId": Uuid::nil(), "Username": "Example User", "NotificationUsername": "Example User", "UserId": Uuid::nil(), "Timestamp": Local::now().to_rfc3339(), "UtcTimestamp": Utc::now().to_rfc3339(), "PlaybackPositionTicks": 0, "PlaybackPosition": "00:00:00" })
}

fn webhook_item_type(kind: &db::MediaKind) -> &'static str {
    match kind {
        db::MediaKind::Movie => "Movie",
        db::MediaKind::Series => "Series",
        db::MediaKind::Season => "Season",
        db::MediaKind::Episode => "Episode",
        db::MediaKind::Track => "Audio",
        db::MediaKind::Album => "MusicAlbum",
        db::MediaKind::Artist => "MusicArtist",
        db::MediaKind::Collection => "BoxSet",
        db::MediaKind::Folder => "Folder",
        db::MediaKind::Playlist => "Playlist",
        db::MediaKind::TvChannel => "TvChannel",
        db::MediaKind::TvProgram => "TvProgram",
        _ => "Unknown",
    }
}

fn format_ticks(ticks: i64) -> String {
    let seconds = ticks.max(0) / 10_000_000;
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

fn playback_context(
    client: &PlaybackContext,
    position_ticks: i64,
    is_paused: bool,
    played_to_completion: Option<bool>,
) -> Value {
    json!({
        "PlaybackPositionTicks": position_ticks,
        "PlaybackPosition": format_ticks(position_ticks),
        "PlayMethod": client.play_method.as_ref().map(ToString::to_string),
        "PlayedToCompletion": played_to_completion,
        "IsPaused": is_paused,
        "IsAutomated": false,
        "SessionId": client.session_id,
        "MediaSourceId": client.media_source_id,
        "DeviceId": client.device_id,
        "DeviceName": client.device_name,
        "ClientName": client.client_name,
        "Client": client.client_name,
        "RemoteEndPoint": client.remote_endpoint,
        "AudioStreamIndex": client.audio_stream_index,
        "SubtitleStreamIndex": client.subtitle_stream_index,
    })
}

async fn event_context(
    ctx: &AppContext,
    event: WebhookEvent,
    user: Option<&db::User>,
    mut media: Option<db::Media>,
    extra: Value,
) -> anyhow::Result<Value> {
    let now = Utc::now();
    let server_name = db::Settings::get_config(&ctx.db)
        .await?
        .server_name
        .unwrap_or_else(|| "Remux".to_string());
    let mut context = json!({
        "ServerId": crate::common::server_id(),
        "ServerName": server_name,
        "ServerVersion": ctx.config.jellyfin_version,
        "ServerUrl": "",
        "NotificationType": event.to_string(),
        "Timestamp": Local::now().to_rfc3339(),
        "UtcTimestamp": now.to_rfc3339(),
    });
    let obj = context
        .as_object_mut()
        .expect("webhook context is an object");

    if let Some(user) = user {
        obj.insert("NotificationUsername".into(), json!(user.username));
        obj.insert("Username".into(), json!(user.username));
        obj.insert("UserId".into(), json!(user.id));
    }

    if let Some(media) = media.as_mut() {
        let runtime_ticks = media
            .runtime
            .map(|seconds| seconds * 10_000_000);
        obj.insert("Name".into(), json!(media.title));
        obj.insert("Overview".into(), json!(media.description));
        obj.insert("ItemId".into(), json!(media.id));
        obj.insert("ItemType".into(), json!(webhook_item_type(&media.kind)));
        obj.insert("RunTimeTicks".into(), json!(runtime_ticks));
        obj.insert("RunTime".into(), json!(runtime_ticks.map(format_ticks)));
        obj.insert(
            "Year".into(),
            json!(
                media
                    .released_at
                    .map(|date| date.year())
            ),
        );
        obj.insert(
            "PremiereDate".into(),
            json!(
                media
                    .released_at
                    .map(|date| date
                        .and_utc()
                        .to_rfc3339())
            ),
        );
        obj.insert("MediaSourceId".into(), json!(media.id));

        let mut provider_ids = serde_json::Map::new();
        if let Some(imdb) = media
            .external_ids
            .imdb
            .as_ref()
        {
            let imdb = imdb.to_string();
            obj.insert("Provider_imdb".into(), json!(imdb));
            provider_ids.insert("Imdb".into(), json!(imdb));
        }
        if let Some(tmdb) = media
            .external_ids
            .tmdb
        {
            obj.insert("Provider_tmdb".into(), json!(tmdb.to_string()));
            provider_ids.insert("Tmdb".into(), json!(tmdb.to_string()));
        }
        if let Some(tvdb) = media
            .external_ids
            .tvdb
        {
            obj.insert("Provider_tvdb".into(), json!(tvdb.to_string()));
            provider_ids.insert("Tvdb".into(), json!(tvdb.to_string()));
        }
        obj.insert("ProviderIds".into(), Value::Object(provider_ids));

        let series_id = match media.kind {
            db::MediaKind::Episode => media.grandparent_id,
            db::MediaKind::Season => media
                .grandparent_id
                .or(media.parent_id),
            _ => None,
        };
        if let Some(series_id) = series_id {
            if let Some(series) = db::Media::get_by_id(&ctx.db, &series_id).await? {
                obj.insert("SeriesId".into(), json!(series.id));
                obj.insert("SeriesName".into(), json!(series.title));
                obj.insert(
                    "SeriesPremiereDate".into(),
                    json!(
                        series
                            .released_at
                            .map(|date| date
                                .and_utc()
                                .to_rfc3339())
                    ),
                );
            }
        }
        if matches!(media.kind, db::MediaKind::Episode | db::MediaKind::Season) {
            let season_number = if media.kind == db::MediaKind::Season {
                media.idx
            } else if let Some(parent_id) = media.parent_id {
                db::Media::get_by_id(&ctx.db, &parent_id)
                    .await?
                    .and_then(|season| season.idx)
                    .or(media.parent_idx)
            } else {
                media.parent_idx
            };
            if let Some(number) = season_number {
                obj.insert("SeasonNumber".into(), json!(number));
                obj.insert("SeasonNumber00".into(), json!(format!("{number:02}")));
                obj.insert("SeasonNumber000".into(), json!(format!("{number:03}")));
            }
        }
        if media.kind == db::MediaKind::Episode {
            if let Some(number) = media.idx {
                obj.insert("EpisodeNumber".into(), json!(number));
                obj.insert("EpisodeNumber00".into(), json!(format!("{number:02}")));
                obj.insert("EpisodeNumber000".into(), json!(format!("{number:03}")));
            }
        }
        if media.kind == db::MediaKind::Track {
            obj.insert("Album".into(), json!(media.album_name()));
            obj.insert("Artist".into(), json!(media.artist_name()));
        } else if media.kind == db::MediaKind::Album {
            obj.insert("Artist".into(), json!(media.artist_name()));
        }

        media
            .load_relations(&ctx.db)
            .await?;
        let genres: Vec<_> = media
            .relations
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|(_, related)| related.kind == db::MediaKind::Genre)
            .map(|(_, related)| {
                related
                    .title
                    .clone()
            })
            .collect();
        obj.insert("Genres".into(), json!(genres));

        if let Some(user) = user {
            if let Some(state) = media
                .user_state(&ctx.db, user)
                .await?
            {
                obj.insert("Favorite".into(), json!(state.favorite));
                obj.insert("Played".into(), json!(state.play_count > 0));
                obj.insert("PlayCount".into(), json!(state.play_count));
                obj.insert("Rating".into(), json!(state.rating));
                obj.insert(
                    "Likes".into(),
                    json!(
                        state
                            .rating
                            .map(|rating| rating >= db::UserRating::LIKE_THRESHOLD)
                    ),
                );
                obj.insert("LastPlayedDate".into(), json!(state.last_played_at));
            }
        }
    }

    if let Some(extra) = extra.as_object() {
        obj.extend(extra.clone());
    }
    Ok(context)
}

fn render_body(
    template: &str,
    all: bool,
    trim: bool,
    context: &Value,
) -> anyhow::Result<String> {
    let mut body = if all
        || template
            .trim()
            .is_empty()
    {
        serde_json::to_string(context)?
    } else {
        let mut handlebars = Handlebars::new();
        handlebars.register_helper("if_equals", Box::new(IfEqualsHelper));
        handlebars.register_helper("if_exist", Box::new(IfExistHelper));
        handlebars.register_helper("link_to", Box::new(LinkToHelper));
        handlebars.register_helper("url_encode", Box::new(UrlEncodeHelper));
        handlebars.register_helper("json_encode", Box::new(JsonEncodeHelper));
        handlebars.render_template(template, context)?
    };
    if trim {
        body = body
            .trim()
            .to_string();
    }
    Ok(body)
}

/// Jellyfin Webhook templates use `if_equals` as a block helper. Handlebars-rust
/// has an `eq` helper, but not the Jellyfin-compatible name or block behavior.
struct IfEqualsHelper;

impl HelperDef for IfEqualsHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        helper: &Helper<'rc>,
        registry: &'reg Handlebars<'reg>,
        context: &'rc Context,
        render_context: &mut RenderContext<'reg, 'rc>,
        output: &mut dyn Output,
    ) -> HelperResult {
        let equal = helper
            .param(0)
            .zip(helper.param(1))
            .is_some_and(|(left, right)| {
                left.value()
                    .render()
                    .eq_ignore_ascii_case(
                        &right
                            .value()
                            .render(),
                    )
            });
        if helper.is_block() {
            let template = if equal {
                helper.template()
            } else {
                helper.inverse()
            };
            if let Some(template) = template {
                template.render(registry, context, render_context, output)?;
            }
        } else {
            output.write(if equal { "true" } else { "false" })?;
        }
        Ok(())
    }
}

struct IfExistHelper;

impl HelperDef for IfExistHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        helper: &Helper<'rc>,
        registry: &'reg Handlebars<'reg>,
        context: &'rc Context,
        render_context: &mut RenderContext<'reg, 'rc>,
        output: &mut dyn Output,
    ) -> HelperResult {
        let exists = helper
            .param(0)
            .is_some_and(|param| {
                !param
                    .value()
                    .is_null()
                    && !param
                        .value()
                        .render()
                        .is_empty()
            });
        if helper.is_block() {
            let template = if exists {
                helper.template()
            } else {
                helper.inverse()
            };
            if let Some(template) = template {
                template.render(registry, context, render_context, output)?;
            }
        } else {
            output.write(if exists { "true" } else { "false" })?;
        }
        Ok(())
    }
}

struct LinkToHelper;

impl HelperDef for LinkToHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        helper: &Helper<'rc>,
        _registry: &'reg Handlebars<'reg>,
        _context: &'rc Context,
        _render_context: &mut RenderContext<'reg, 'rc>,
        output: &mut dyn Output,
    ) -> HelperResult {
        let url = helper
            .hash_get("url")
            .or_else(|| helper.param(0))
            .map(|value| {
                value
                    .value()
                    .render()
            })
            .unwrap_or_default();
        let text = helper
            .hash_get("text")
            .or_else(|| helper.param(1))
            .map(|value| {
                value
                    .value()
                    .render()
            })
            .unwrap_or_default();
        output.write(&format!("<a href='{url}'>{text}</a>"))?;
        Ok(())
    }
}

struct UrlEncodeHelper;

impl HelperDef for UrlEncodeHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        helper: &Helper<'rc>,
        _registry: &'reg Handlebars<'reg>,
        _context: &'rc Context,
        _render_context: &mut RenderContext<'reg, 'rc>,
        output: &mut dyn Output,
    ) -> HelperResult {
        let value = helper
            .param(0)
            .map(|param| {
                param
                    .value()
                    .render()
            })
            .unwrap_or_default();
        let encoded: String =
            url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
        output.write(&encoded)?;
        Ok(())
    }
}

struct JsonEncodeHelper;

impl HelperDef for JsonEncodeHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        helper: &Helper<'rc>,
        _registry: &'reg Handlebars<'reg>,
        _context: &'rc Context,
        _render_context: &mut RenderContext<'reg, 'rc>,
        output: &mut dyn Output,
    ) -> HelperResult {
        let value = helper
            .param(0)
            .map(|param| {
                param
                    .value()
                    .render()
            })
            .unwrap_or_default();
        let encoded = serde_json::to_string(&value)
            .unwrap_or_default()
            .trim_matches('"')
            .to_string();
        output.write(&encoded)?;
        Ok(())
    }
}

async fn send_webhook(
    config: &Webhook,
    event: WebhookEvent,
    mut context: Value,
) -> anyhow::Result<String> {
    if !config.enabled {
        tracing::debug!(webhook_id = %config.id, event = %event, "webhook disabled");
        return Ok(String::new());
    }
    if !config
        .events
        .is_empty()
        && !config
            .events
            .contains(&event)
    {
        tracing::debug!(webhook_id = %config.id, event = %event, configured_events = ?config.events, "webhook event not selected");
        return Ok(String::new());
    }
    if let Some(obj) = context.as_object_mut() {
        for (k, v) in &config
            .fields
            .0
        {
            obj.insert(k.clone(), Value::String(v.clone()));
        }
    }
    let body = render_body(
        &config.template,
        config.send_all_properties,
        config.trim_whitespace,
        &context,
    )?;
    if config.skip_empty_body
        && body
            .trim()
            .is_empty()
    {
        return Ok(body);
    }

    match &config
        .destination
        .0
    {
        WebhookDestination::Http(destination) => {
            deliver_http(destination, &body, config.id, event).await?
        }
    }
    Ok(body)
}

async fn deliver_http(
    destination: &HttpWebhookConfig,
    body: &str,
    webhook_id: Uuid,
    event: WebhookEvent,
) -> anyhow::Result<()> {
    let mut request = WEBHOOK_CLIENT
        .post(&destination.url)
        .body(body.to_owned());
    let mut content_type = "text/plain".to_string();
    for (key, value) in &destination.headers {
        if key.eq_ignore_ascii_case("content-type") {
            content_type = value.clone();
            continue;
        }
        request =
            request.header(HeaderName::try_from(key)?, HeaderValue::try_from(value)?);
    }
    request = request.header("content-type", content_type);
    let result = request
        .send()
        .await;
    match result {
        Ok(response) => {
            let status = response.status();
            if !status.is_success() {
                anyhow::bail!("webhook returned HTTP {status}");
            }
            tracing::debug!(webhook_id = %webhook_id, event = %event, status = status.as_u16(), "webhook delivered");
        }
        Err(error) => {
            return Err(error.into());
        }
    }
    Ok(())
}

pub struct WebhookSubscriber {
    pub ctx: crate::AppContext,
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{Method::POST, MockServer};

    #[test]
    fn jellyfin_if_equals_helper_renders_block_branches() {
        let context = sample_context(WebhookEvent::PlaybackStart);
        let matching = render_body(
            r#"{{#if_equals NotificationType "PlaybackStart"}}yes{{else}}no{{/if_equals}}"#,
            false,
            false,
            &context,
        )
        .unwrap();
        let non_matching = render_body(
            r#"{{#if_equals NotificationType "PlaybackStop"}}yes{{else}}no{{/if_equals}}"#,
            false,
            false,
            &context,
        )
        .unwrap();
        let bool_matching = render_body(
            r#"{{#if_equals PlayedToCompletion "True"}}yes{{else}}no{{/if_equals}}"#,
            false,
            false,
            &json!({ "PlayedToCompletion": true }),
        )
        .unwrap();
        assert_eq!(matching, "yes");
        assert_eq!(non_matching, "no");
        assert_eq!(bool_matching, "yes");
    }

    #[test]
    fn jellyfin_link_to_helper_reads_hash_arguments() {
        let body = render_body(
            r#"{{{link_to url="https://example.com/item" text="Open item"}}}"#,
            false,
            false,
            &json!({}),
        )
        .unwrap();
        assert_eq!(body, "<a href='https://example.com/item'>Open item</a>");
    }

    #[tokio::test]
    async fn http_destination_sends_configured_headers() {
        let server = MockServer::start_async().await;
        let request = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/hook")
                    .header("authorization", "Bearer secret")
                    .header("content-type", "application/json");
                then.status(204);
            })
            .await;
        let destination = HttpWebhookConfig {
            url: server.url("/hook"),
            headers: HashMap::from([
                ("Authorization".to_string(), "Bearer secret".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ]),
        };

        deliver_http(
            &destination,
            r#"{"event":"Play"}"#,
            Uuid::nil(),
            WebhookEvent::PlaybackStart,
        )
        .await
        .unwrap();

        request
            .assert_async()
            .await;
    }
}

#[async_trait]
impl Subscriber for WebhookSubscriber {
    fn key(&self) -> &'static str {
        "webhooks"
    }
    fn events(&self) -> &[EventType] {
        &[
            EventType::PlaybackStarted,
            EventType::PlaybackProgress,
            EventType::PlaybackStopped,
            EventType::MarkPlayed,
            EventType::MarkUnplayed,
            EventType::MarkFavorite,
            EventType::UnmarkFavorite,
            EventType::Rating,
            EventType::UserUpdated,
            EventType::UserDeleted,
        ]
    }
    async fn handle(&self, event: Event) -> anyhow::Result<()> {
        let (event_name, user_id, media_id, extra) = match event {
            Event::PlaybackStarted(i) => (
                WebhookEvent::PlaybackStart,
                Some(i.user_id),
                Some(i.media_id),
                playback_context(&i, i.position_ticks, false, None),
            ),
            Event::PlaybackProgress(i) => (
                WebhookEvent::PlaybackProgress,
                Some(i.user_id),
                Some(i.media_id),
                playback_context(&i, i.position_ticks, i.is_paused, None),
            ),
            Event::PlaybackStopped(i) => (
                WebhookEvent::PlaybackStop,
                Some(i.user_id),
                Some(i.media_id),
                playback_context(&i, i.position_ticks, false, Some(i.played)),
            ),
            Event::MarkPlayed(i) => (
                WebhookEvent::UserDataSaved,
                Some(i.user_id),
                Some(i.media_id),
                json!({"Played":true,"SaveReason":"TogglePlayed"}),
            ),
            Event::MarkUnplayed(i) => (
                WebhookEvent::UserDataSaved,
                Some(i.user_id),
                Some(i.media_id),
                json!({"Played":false,"SaveReason":"TogglePlayed"}),
            ),
            Event::MarkFavorite(i) => (
                WebhookEvent::UserDataSaved,
                Some(i.user_id),
                Some(i.media_id),
                json!({"Favorite":true,"SaveReason":"ToggleFavorite"}),
            ),
            Event::UnmarkFavorite(i) => (
                WebhookEvent::UserDataSaved,
                Some(i.user_id),
                Some(i.media_id),
                json!({"Favorite":false,"SaveReason":"ToggleFavorite"}),
            ),
            Event::Rating(i) => (
                WebhookEvent::UserDataSaved,
                Some(i.user_id),
                Some(i.media_id),
                json!({"Rating":i.rating,"SaveReason":"UpdateUserRating"}),
            ),
            Event::UserUpdated(i) => {
                (WebhookEvent::UserUpdated, Some(i.user_id), None, json!({}))
            }
            Event::UserDeleted(i) => {
                (WebhookEvent::UserDeleted, Some(i.user_id), None, json!({}))
            }
            _ => return Ok(()),
        };
        let configs = Webhook::list(
            &self
                .ctx
                .db,
        )
        .await?;
        tracing::debug!(
            event = %event_name,
            webhook_count = configs.len(),
            "processing webhook event"
        );
        for config in configs {
            if user_id.is_some_and(|id| {
                !config
                    .user_ids
                    .is_empty()
                    && !config
                        .user_ids
                        .contains(&id)
            }) {
                continue;
            }
            let media = if let Some(id) = media_id {
                db::Media::get_by_id(
                    &self
                        .ctx
                        .db,
                    &id,
                )
                .await?
            } else {
                None
            };
            if !config
                .media_types
                .is_empty()
            {
                let Some(media) = media.as_ref() else {
                    continue;
                };
                if !config
                    .media_types
                    .iter()
                    .any(|kind| {
                        kind.eq_ignore_ascii_case(
                            &media
                                .kind
                                .to_string(),
                        )
                    })
                {
                    continue;
                }
            }
            let user = if let Some(id) = user_id {
                db::User::get_by_id(
                    &self
                        .ctx
                        .db,
                    &id,
                )
                .await?
            } else {
                None
            };
            let context = event_context(
                &self.ctx,
                event_name,
                user.as_ref(),
                media,
                extra.clone(),
            )
            .await?;
            if let Err(error) = send_webhook(&config, event_name, context).await {
                tracing::warn!(webhook_id=%config.id, event=%event_name, %error, "webhook delivery failed");
            }
        }
        Ok(())
    }
}
