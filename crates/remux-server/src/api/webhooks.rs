//! Admin CRUD over outgoing webhooks, plus the synchronous "test this webhook"
//! endpoint the dashboard uses for immediate feedback.
//!
//! **Every route is admin-only.** A webhook URL is a credential — Discord's is
//! `https://discord.com/api/webhooks/{id}/{token}` — so read access is as
//! sensitive as write access. `auth::AdminSession` in the signature is the
//! whole mechanism.
//!
//! **Every mutation ends in `state.ctx.webhooks.invalidate()`.** The dispatcher
//! caches the enabled hook set and reloads only when that flag is set, so a
//! write that skips the call does nothing until the process restarts.

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use http::StatusCode;
use remux_macros::{delete, get, post};
use remux_sdks::remux::{WebhookDto, WebhookTestResult};
use std::str::FromStr;
use url::Url;
use uuid::Uuid;

use crate::{
    AppState, IntoApiError, OptionExt,
    db::{self, auth},
    services::webhooks,
};
use axum_anyhow::ApiResult as Result;

/// A URL the server is willing to POST a webhook to.
///
/// Parse, don't validate: nothing downstream has to re-check. The scheme
/// restriction is load-bearing — `Url::parse` accepts `file:///etc/shadow` and
/// `javascript:alert(1)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookUrl(Url);

impl WebhookUrl {
    /// The canonical serialization, not the operator's raw string.
    fn into_stored(self) -> String {
        self.0
            .into()
    }
}

/// Why a webhook URL was refused. No variant embeds the offending URL: the
/// message travels back to the browser and into logs, and the URL is a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WebhookUrlError {
    #[error("webhook url must be an absolute URL")]
    Malformed,
    #[error("webhook url must use http or https")]
    UnsupportedScheme,
    #[error("webhook url must have a host")]
    MissingHost,
}

impl FromStr for WebhookUrl {
    type Err = WebhookUrlError;

    fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
        let url = Url::parse(raw.trim()).map_err(|_| WebhookUrlError::Malformed)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(WebhookUrlError::UnsupportedScheme);
        }
        if url
            .host_str()
            .is_none_or(str::is_empty)
        {
            return Err(WebhookUrlError::MissingHost);
        }
        Ok(Self(url))
    }
}

/// `payload` with its URL replaced by the parsed, canonical form — or a 400.
fn with_parsed_url(payload: WebhookDto) -> Result<WebhookDto> {
    match payload
        .url
        .parse::<WebhookUrl>()
    {
        Ok(url) => Ok(WebhookDto {
            url: url.into_stored(),
            ..payload
        }),
        Err(e) => {
            let detail = e.to_string();
            Err(e.context_bad_request(&detail))
        }
    }
}

/// `payload` with its template proved to parse — or a 400 carrying handlebars'
/// own message.
///
/// The parse error is derived from the operator's own template — never from a
/// remote response, never from the URL — so returning it leaks nothing.
///
/// Checked even when `send_all_properties` bypasses the template at render
/// time: the flag is one checkbox away from being turned off.
fn with_checked_template(payload: WebhookDto) -> Result<WebhookDto> {
    match webhooks::validate_template(&payload.template) {
        Ok(()) => Ok(payload),
        Err(e) => {
            let detail = format!("webhook template does not parse: {e}");
            Err(e.context_bad_request(&detail))
        }
    }
}

/// The stored webhook, or a 404 — so a missing row is not a 500 out of the
/// repository's re-read.
async fn load(state: &AppState, id: &Uuid) -> Result<db::Webhook> {
    db::Webhook::get_by_id(
        &state
            .ctx
            .db,
        id,
    )
    .await?
    .context_not_found("webhook not found")
}

/// List every webhook, enabled or not.
#[get("/remux/webhooks")]
pub async fn get_webhooks(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    let hooks = db::Webhook::get_all(
        &state
            .ctx
            .db,
    )
    .await?;
    let dtos: Vec<WebhookDto> = hooks
        .into_iter()
        .map(db::Webhook::into_dto)
        .collect();
    Ok(Json(dtos))
}

/// Read one webhook by id.
#[get("/remux/webhooks/{id}")]
pub async fn get_webhook(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let hook = load(&state, &id).await?;
    Ok(Json(hook.into_dto()))
}

/// Create a webhook. The id carried by the payload is ignored.
#[post("/remux/webhooks")]
pub async fn create_webhook(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Json(payload): Json<WebhookDto>,
) -> Result<impl IntoResponse> {
    let payload = with_checked_template(with_parsed_url(payload)?)?;
    let created = db::Webhook::create(
        &state
            .ctx
            .db,
        &payload,
    )
    .await?;
    state
        .ctx
        .webhooks
        .invalidate();
    Ok(Json(created.into_dto()))
}

/// Replace every mutable field of a webhook. POST, not PUT — the SDK's
/// `UpdateWebhook` endpoint declares POST and that contract is already merged.
#[post("/remux/webhooks/{id}")]
pub async fn update_webhook(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<WebhookDto>,
) -> Result<impl IntoResponse> {
    load(&state, &id).await?;
    let payload = with_checked_template(with_parsed_url(payload)?)?;
    let updated = db::Webhook::update(
        &state
            .ctx
            .db,
        &id,
        &payload,
    )
    .await?;
    state
        .ctx
        .webhooks
        .invalidate();
    Ok(Json(updated.into_dto()))
}

/// Delete a webhook.
#[delete("/remux/webhooks/{id}")]
pub async fn delete_webhook(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    load(&state, &id).await?;
    db::Webhook::delete(
        &state
            .ctx
            .db,
        &id,
    )
    .await?;
    state
        .ctx
        .webhooks
        .invalidate();
    Ok(StatusCode::NO_CONTENT)
}

/// Send one synthetic `Generic` event to a webhook, right now, and report what
/// the endpoint said.
///
/// Synchronous and outside the broadcast channel on purpose — see
/// [`webhooks::deliver_test`]. A refusing or unreachable endpoint is a `200`
/// carrying `success: false`: the *request* worked, the *test* did not.
#[post("/remux/webhooks/{id}/test")]
pub async fn test_webhook(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let hook = load(&state, &id).await?;
    let result: WebhookTestResult = webhooks::deliver_test(&state.ctx, &hook).await;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        integration_test::{
            AUTH_HEADER, auth_header_with_token, authenticated_server, new_test_server,
        },
        services::webhooks::WebhookEvent,
    };
    use axum_test::TestServer;
    use http::header::{HeaderName, HeaderValue};
    use httpmock::{Method::POST, Mock, MockServer};
    use remux_sdks::remux::{
        DiscordMentionType, NotificationType, WebhookDestination, WebhookItemTypes,
        WebhookKeyValue,
    };
    use serde_json::json;
    use std::time::{Duration, Instant};

    /// Valid JSON echoing exactly one variable, so a received request pins both
    /// the template output and the variable dictionary.
    const TEMPLATE: &str = r#"{"content":"{{Name}}"}"#;

    fn auth(token: &str) -> (HeaderName, HeaderValue) {
        (
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&auth_header_with_token(token)).unwrap(),
        )
    }

    /// `id` is deliberately non-nil so the round-trip proves the server assigns
    /// its own.
    fn hook_dto(name: &str, url: &str) -> WebhookDto {
        WebhookDto {
            id: Uuid::from_u128(0xdead_beef),
            name: name.into(),
            enabled: true,
            url: url.into(),
            template: TEMPLATE.into(),
            destination: WebhookDestination::Generic {
                headers: vec![],
                fields: vec![],
            },
            notification_types: vec![NotificationType::Generic],
            user_filter: vec![],
            item_types: WebhookItemTypes::default(),
            send_all_properties: false,
            trim_whitespace: false,
            skip_empty_message_body: false,
            created_at: None,
            updated_at: None,
        }
    }

    async fn create(
        server: &TestServer,
        h: &HeaderName,
        v: &HeaderValue,
        dto: &WebhookDto,
    ) -> WebhookDto {
        server
            .post("/remux/webhooks")
            .add_header(h.clone(), v.clone())
            .json(dto)
            .await
            .json()
    }

    async fn list(
        server: &TestServer,
        h: &HeaderName,
        v: &HeaderValue,
    ) -> Vec<WebhookDto> {
        server
            .get("/remux/webhooks")
            .add_header(h.clone(), v.clone())
            .await
            .json()
    }

    fn generic_event() -> WebhookEvent {
        WebhookEvent::Generic {
            title: "dispatcher probe".into(),
            extra: vec![],
        }
    }

    /// Poll `condition` until it holds, failing the test rather than hanging.
    async fn eventually(what: &str, mut condition: impl AsyncFnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition().await {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Give an unwanted delivery every chance to arrive before asserting it did
    /// not: the canary only proves the event was *dispatched*.
    async fn settle() {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    async fn hits(mock: &Mock<'_>) -> usize {
        mock.hits_async()
            .await
    }

    // --- WebhookUrl -------------------------------------------------------

    #[test]
    fn a_webhook_url_accepts_http_and_https_and_canonicalises() {
        for (raw, stored) in [
            ("https://example.test/hook", "https://example.test/hook"),
            ("http://example.test/hook", "http://example.test/hook"),
            // Trimmed, and given the path `Url` considers canonical.
            ("  https://example.test  ", "https://example.test/"),
            (
                "https://discord.com/api/webhooks/1/tok?wait=true",
                "https://discord.com/api/webhooks/1/tok?wait=true",
            ),
        ] {
            let parsed: WebhookUrl = raw
                .parse()
                .unwrap_or_else(|e| panic!("{raw} must parse: {e}"));
            assert_eq!(parsed.into_stored(), stored);
        }
    }

    #[test]
    fn a_webhook_url_rejects_what_cannot_be_posted_to() {
        for (raw, expected) in [
            ("", WebhookUrlError::Malformed),
            ("not a url", WebhookUrlError::Malformed),
            ("example.test/hook", WebhookUrlError::Malformed),
            ("/hook", WebhookUrlError::Malformed),
            ("file:///etc/shadow", WebhookUrlError::UnsupportedScheme),
            ("javascript:alert(1)", WebhookUrlError::UnsupportedScheme),
            (
                "ftp://example.test/hook",
                WebhookUrlError::UnsupportedScheme,
            ),
        ] {
            assert_eq!(
                raw.parse::<WebhookUrl>(),
                Err(expected),
                "{raw} must be refused"
            );
        }
    }

    /// The rejection travels back to the browser and into logs.
    #[test]
    fn a_url_rejection_never_echoes_the_url() {
        let secret = "gopher://discord.com/api/webhooks/1/aVerySecretToken";
        let message = secret
            .parse::<WebhookUrl>()
            .expect_err("gopher is not a webhook scheme")
            .to_string();
        assert!(!message.contains("aVerySecretToken"), "{message}");
        assert!(!message.contains("discord.com"), "{message}");
    }

    // --- CRUD -------------------------------------------------------------

    #[tokio::test]
    async fn crud_round_trip_over_http() {
        let (server, _guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        assert!(
            list(&server, &h, &v)
                .await
                .is_empty(),
            "a fresh server has no webhooks"
        );

        let payload = hook_dto("discord", "https://example.test/hook");
        let created = create(&server, &h, &v, &payload).await;
        assert_ne!(created.id, payload.id, "the server assigns the id");
        assert_eq!(created.name, "discord");
        assert_eq!(created.url, "https://example.test/hook");
        assert_eq!(created.template, TEMPLATE);
        assert!(created.enabled);
        assert!(
            created
                .created_at
                .is_some(),
            "the stored timestamps come back to the dashboard"
        );

        let all = list(&server, &h, &v).await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, created.id);

        let fetched: WebhookDto = server
            .get(&format!("/remux/webhooks/{}", created.id))
            .add_header(h.clone(), v.clone())
            .await
            .json();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, "discord");

        let update = WebhookDto {
            name: "renamed".into(),
            enabled: false,
            notification_types: vec![NotificationType::ItemAdded],
            destination: WebhookDestination::Generic {
                headers: vec![WebhookKeyValue {
                    key: "X-Auth-Token".into(),
                    value: "s3cret".into(),
                }],
                fields: vec![],
            },
            ..hook_dto("renamed", "https://example.test/other")
        };
        let updated: WebhookDto = server
            .post(&format!("/remux/webhooks/{}", created.id))
            .add_header(h.clone(), v.clone())
            .json(&update)
            .await
            .json();
        assert_eq!(updated.id, created.id, "update must not re-key the row");
        assert_eq!(updated.name, "renamed");
        assert_eq!(updated.url, "https://example.test/other");
        assert!(!updated.enabled);
        assert_eq!(
            updated.notification_types,
            vec![NotificationType::ItemAdded]
        );
        assert_eq!(updated.destination, update.destination);

        let refetched: WebhookDto = server
            .get(&format!("/remux/webhooks/{}", created.id))
            .add_header(h.clone(), v.clone())
            .await
            .json();
        assert_eq!(refetched.name, "renamed");
        assert_eq!(refetched.destination, update.destination);

        server
            .delete(&format!("/remux/webhooks/{}", created.id))
            .add_header(h.clone(), v.clone())
            .await
            .assert_status(StatusCode::NO_CONTENT);

        assert!(
            list(&server, &h, &v)
                .await
                .is_empty(),
            "the webhook is gone after the delete"
        );
        server
            .get(&format!("/remux/webhooks/{}", created.id))
            .add_header(h, v)
            .expect_failure()
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_unknown_id_is_a_404_on_every_route_that_takes_one() {
        let (server, _guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let missing = Uuid::new_v4();

        server
            .get(&format!("/remux/webhooks/{missing}"))
            .add_header(h.clone(), v.clone())
            .expect_failure()
            .await
            .assert_status(StatusCode::NOT_FOUND);

        server
            .post(&format!("/remux/webhooks/{missing}"))
            .add_header(h.clone(), v.clone())
            .expect_failure()
            .json(&hook_dto("ghost", "https://example.test/hook"))
            .await
            .assert_status(StatusCode::NOT_FOUND);

        server
            .delete(&format!("/remux/webhooks/{missing}"))
            .add_header(h.clone(), v.clone())
            .expect_failure()
            .await
            .assert_status(StatusCode::NOT_FOUND);

        server
            .post(&format!("/remux/webhooks/{missing}/test"))
            .add_header(h, v)
            .expect_failure()
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    // --- url validation ---------------------------------------------------

    #[tokio::test]
    async fn a_url_that_does_not_parse_is_rejected_on_create_and_on_update() {
        let (server, _guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        for bad in [
            "not a url",
            "",
            "example.test/hook",
            "file:///etc/shadow",
            "javascript:alert(1)",
        ] {
            server
                .post("/remux/webhooks")
                .add_header(h.clone(), v.clone())
                .expect_failure()
                .json(&hook_dto("bad", bad))
                .await
                .assert_status(StatusCode::BAD_REQUEST);
        }
        assert!(
            list(&server, &h, &v)
                .await
                .is_empty(),
            "a rejected create must not store anything"
        );

        let created = create(
            &server,
            &h,
            &v,
            &hook_dto("good", "https://example.test/hook"),
        )
        .await;
        server
            .post(&format!("/remux/webhooks/{}", created.id))
            .add_header(h.clone(), v.clone())
            .expect_failure()
            .json(&hook_dto("bad", "not a url"))
            .await
            .assert_status(StatusCode::BAD_REQUEST);

        let unchanged: WebhookDto = server
            .get(&format!("/remux/webhooks/{}", created.id))
            .add_header(h, v)
            .await
            .json();
        assert_eq!(
            unchanged.url, "https://example.test/hook",
            "a rejected update must not touch the stored row"
        );
    }

    // --- template validation ----------------------------------------------

    /// The write is refused with the parse error, which comes from the
    /// operator's own template and not from any remote response.
    #[tokio::test]
    async fn a_template_that_does_not_parse_is_rejected_on_create_and_on_update() {
        let (server, _guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        let broken = WebhookDto {
            template: "{{#if_equals ItemType 'Movie'}}unclosed".into(),
            ..hook_dto("broken", "https://example.test/hook")
        };
        server
            .post("/remux/webhooks")
            .add_header(h.clone(), v.clone())
            .expect_failure()
            .json(&broken)
            .await
            .assert_status(StatusCode::BAD_REQUEST);
        assert!(
            list(&server, &h, &v)
                .await
                .is_empty(),
            "a rejected create must not store anything"
        );

        let created = create(
            &server,
            &h,
            &v,
            &hook_dto("good", "https://example.test/hook"),
        )
        .await;
        server
            .post(&format!("/remux/webhooks/{}", created.id))
            .add_header(h.clone(), v.clone())
            .expect_failure()
            .json(&broken)
            .await
            .assert_status(StatusCode::BAD_REQUEST);

        let unchanged: WebhookDto = server
            .get(&format!("/remux/webhooks/{}", created.id))
            .add_header(h, v)
            .await
            .json();
        assert_eq!(
            unchanged.template, TEMPLATE,
            "a rejected update must not touch the stored row"
        );
    }

    /// The template the dashboard pre-fills has to be acceptable here.
    #[tokio::test]
    async fn the_stock_discord_template_is_accepted() {
        let (server, _guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        let dto = WebhookDto {
            template: remux_sdks::remux::DISCORD_TEMPLATE.into(),
            ..hook_dto("discord", "https://example.test/hook")
        };
        server
            .post("/remux/webhooks")
            .add_header(h, v)
            .json(&dto)
            .await
            .assert_status_ok();
    }

    // --- authorization ----------------------------------------------------

    #[tokio::test]
    async fn every_route_requires_a_session() {
        let (server, _guard) = new_test_server()
            .await
            .unwrap();
        let id = Uuid::new_v4();

        for response in [
            server
                .get("/remux/webhooks")
                .expect_failure()
                .await,
            server
                .get(&format!("/remux/webhooks/{id}"))
                .expect_failure()
                .await,
            server
                .post("/remux/webhooks")
                .expect_failure()
                .json(&hook_dto("x", "https://example.test/hook"))
                .await,
            server
                .post(&format!("/remux/webhooks/{id}"))
                .expect_failure()
                .json(&hook_dto("x", "https://example.test/hook"))
                .await,
            server
                .delete(&format!("/remux/webhooks/{id}"))
                .expect_failure()
                .await,
            server
                .post(&format!("/remux/webhooks/{id}/test"))
                .expect_failure()
                .await,
        ] {
            response.assert_status(StatusCode::UNAUTHORIZED);
        }
    }

    /// A webhook URL embeds a credential, so a non-admin must not be able to
    /// read one — not through the list, not through a by-id read.
    #[tokio::test]
    async fn a_non_admin_cannot_read_or_write_webhooks() {
        let (server, _guard, admin_token) = authenticated_server().await;
        let (h, v) = auth(&admin_token);

        let created = create(
            &server,
            &h,
            &v,
            &hook_dto("secret", "https://discord.test/api/webhooks/1/s3cret"),
        )
        .await;

        server
            .post("/users/new")
            .add_header(h.clone(), v.clone())
            .json(&json!({ "Name": "viewer", "Password": "pass1234" }))
            .await
            .assert_status_ok();
        let token = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER),
            )
            .json(&json!({ "Username": "viewer", "Pw": "pass1234" }))
            .await
            .json::<serde_json::Value>()["AccessToken"]
            .as_str()
            .unwrap()
            .to_string();
        let (uh, uv) = auth(&token);

        for response in [
            server
                .get("/remux/webhooks")
                .add_header(uh.clone(), uv.clone())
                .expect_failure()
                .await,
            server
                .get(&format!("/remux/webhooks/{}", created.id))
                .add_header(uh.clone(), uv.clone())
                .expect_failure()
                .await,
            server
                .post("/remux/webhooks")
                .add_header(uh.clone(), uv.clone())
                .expect_failure()
                .json(&hook_dto("x", "https://example.test/hook"))
                .await,
            server
                .post(&format!("/remux/webhooks/{}", created.id))
                .add_header(uh.clone(), uv.clone())
                .expect_failure()
                .json(&hook_dto("x", "https://example.test/hook"))
                .await,
            server
                .delete(&format!("/remux/webhooks/{}", created.id))
                .add_header(uh.clone(), uv.clone())
                .expect_failure()
                .await,
            server
                .post(&format!("/remux/webhooks/{}/test", created.id))
                .add_header(uh, uv)
                .expect_failure()
                .await,
        ] {
            response.assert_status(StatusCode::UNAUTHORIZED);
            assert!(
                !response
                    .text()
                    .contains("s3cret"),
                "the rejection must not echo the webhook URL"
            );
        }
    }

    // --- the test endpoint ------------------------------------------------

    /// The hook here is disabled and subscribes to nothing, so the dispatcher
    /// would never deliver to it — it must still be testable.
    #[tokio::test]
    async fn the_test_endpoint_delivers_once_and_reports_the_status() {
        let (server, _guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let endpoint_server = MockServer::start_async().await;
        let endpoint = endpoint_server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/hook")
                    .header("x-auth-token", "s3cret")
                    .header("content-type", "application/json; charset=utf-8")
                    .body(r#"{"content":"Test notification"}"#);
                then.status(202);
            })
            .await;

        let dto = WebhookDto {
            enabled: false,
            notification_types: vec![],
            destination: WebhookDestination::Generic {
                headers: vec![WebhookKeyValue {
                    key: "X-Auth-Token".into(),
                    value: "s3cret".into(),
                }],
                fields: vec![],
            },
            ..hook_dto("under test", &endpoint_server.url("/hook"))
        };
        let created = create(&server, &h, &v, &dto).await;

        let result: WebhookTestResult = server
            .post(&format!("/remux/webhooks/{}/test", created.id))
            .add_header(h, v)
            .await
            .json();

        endpoint
            .assert_hits_async(1)
            .await;
        assert!(result.success, "202 is a success: {result:?}");
        assert_eq!(result.status_code, Some(202));
        assert_eq!(result.error, None);
    }

    /// The remote's **response body** must not come back: the URL is
    /// admin-controlled and unrestricted by host, so echoing it would make this
    /// route a read primitive. Asserted on the raw HTTP response, not just the
    /// parsed field.
    #[tokio::test]
    async fn the_test_endpoint_reports_a_rejecting_endpoint_without_retrying() {
        let (server, _guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let endpoint_server = MockServer::start_async().await;
        let leak = "consul-token=s3cret internal detail";
        let endpoint = endpoint_server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/hook");
                then.status(500)
                    .body(leak);
            })
            .await;

        let created = create(
            &server,
            &h,
            &v,
            &hook_dto("under test", &endpoint_server.url("/hook")),
        )
        .await;

        let response = server
            .post(&format!("/remux/webhooks/{}/test", created.id))
            .add_header(h, v)
            .await;
        let raw = response.text();
        let result: WebhookTestResult = response.json();

        assert!(!result.success);
        assert_eq!(result.status_code, Some(500));
        let error = result
            .error
            .clone()
            .expect("a failed test must carry an error");
        assert!(error.contains("500"), "{error}");
        assert!(
            !raw.contains("consul-token"),
            "the remote response body must not reach the admin API: {raw}"
        );
        assert!(!raw.contains("internal detail"), "{raw}");
        endpoint
            .assert_hits_async(1)
            .await;
    }

    /// An unreachable endpoint must not hang the handler or leak the URL path.
    #[tokio::test]
    async fn the_test_endpoint_reports_an_unreachable_endpoint() {
        let (server, _guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        let created = create(
            &server,
            &h,
            &v,
            &hook_dto("dead", "http://127.0.0.1:1/api/webhooks/1/s3cret"),
        )
        .await;

        let result: WebhookTestResult = server
            .post(&format!("/remux/webhooks/{}/test", created.id))
            .add_header(h, v)
            .await
            .json();

        assert!(!result.success);
        assert_eq!(result.status_code, None);
        let error = result
            .error
            .expect("a failed test must carry an error");
        assert!(
            !error.contains("s3cret"),
            "the URL path is a credential and must not be echoed: {error}"
        );
    }

    // --- enrichment failures ----------------------------------------------

    /// An item-scoped event whose item cannot be resolved must not be delivered
    /// at all: `matches` only applies the item-type rule when it is handed a
    /// kind, and enrichment is where the kind comes from.
    ///
    /// The canary is subscribed to a different, itemless event and is the
    /// synchronisation point: once it has been hit, the dispatcher is past the
    /// `ItemAdded` that preceded it, so the negative assertion is not a race.
    #[tokio::test]
    async fn an_item_event_whose_item_cannot_be_resolved_is_not_delivered() {
        let (server, guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let endpoint_server = MockServer::start_async().await;

        let canary_ep = endpoint_server.mock(|when, then| {
            when.method(POST)
                .path("/canary");
            then.status(200);
        });
        let unticked_ep = endpoint_server.mock(|when, then| {
            when.method(POST)
                .path("/unticked");
            then.status(200);
        });

        create(
            &server,
            &h,
            &v,
            &hook_dto("canary", &endpoint_server.url("/canary")),
        )
        .await;
        create(
            &server,
            &h,
            &v,
            &WebhookDto {
                notification_types: vec![NotificationType::ItemAdded],
                // This hook wants no item type at all.
                item_types: WebhookItemTypes {
                    movies: false,
                    episodes: false,
                    series: false,
                    seasons: false,
                    albums: false,
                    songs: false,
                    videos: false,
                },
                ..hook_dto("unticked", &endpoint_server.url("/unticked"))
            },
        )
        .await;

        guard
            .0
            .webhooks
            .emit(WebhookEvent::ItemAdded {
                item_id: Uuid::from_u128(0xf00d),
            });
        guard
            .0
            .webhooks
            .emit(generic_event());

        eventually(
            "the dispatcher to get past the unresolvable item",
            async || hits(&canary_ep).await == 1,
        )
        .await;
        settle().await;
        assert_eq!(
            hits(&unticked_ep).await,
            0,
            "an event with no resolvable item must not slip past the item-type filter"
        );
    }

    // --- dispatcher cache invalidation ------------------------------------

    /// `invalidate()` is how a saved webhook reaches the *running* dispatcher,
    /// so this drives the real cycle: write over HTTP, emit an event, watch the
    /// socket.
    ///
    /// The canary hook is never touched after its creation. Its hit count is
    /// the synchronisation point: once it has seen event N, the dispatcher has
    /// finished dispatching event N, so the negative assertions are not races.
    #[tokio::test]
    async fn create_update_and_delete_each_reach_the_running_dispatcher() {
        let (server, guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let endpoint_server = MockServer::start_async().await;

        let mut endpoint = |path: &'static str| {
            endpoint_server.mock(|when, then| {
                when.method(POST)
                    .path(path);
                then.status(200);
            })
        };
        let canary_ep = endpoint("/canary");
        let first_ep = endpoint("/first");
        let second_ep = endpoint("/second");

        create(
            &server,
            &h,
            &v,
            &hook_dto("canary", &endpoint_server.url("/canary")),
        )
        .await;
        let created = create(
            &server,
            &h,
            &v,
            &hook_dto("under test", &endpoint_server.url("/first")),
        )
        .await;

        // 1. create — the dispatcher booted with an empty cache.
        guard
            .0
            .webhooks
            .emit(generic_event());
        eventually("the created webhooks to be delivered", async || {
            hits(&canary_ep).await == 1 && hits(&first_ep).await == 1
        })
        .await;

        // 2. update — the new URL is only reachable through a reload.
        server
            .post(&format!("/remux/webhooks/{}", created.id))
            .add_header(h.clone(), v.clone())
            .json(&hook_dto("under test", &endpoint_server.url("/second")))
            .await
            .assert_status_ok();
        guard
            .0
            .webhooks
            .emit(generic_event());
        eventually("the updated webhook to be delivered", async || {
            hits(&canary_ep).await == 2 && hits(&second_ep).await == 1
        })
        .await;
        settle().await;
        assert_eq!(
            hits(&first_ep).await,
            1,
            "the pre-update URL must not be posted to again"
        );

        // 3. delete — the hook must stop being delivered to.
        server
            .delete(&format!("/remux/webhooks/{}", created.id))
            .add_header(h, v)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        guard
            .0
            .webhooks
            .emit(generic_event());
        eventually("the canary to see the third event", async || {
            hits(&canary_ep).await == 3
        })
        .await;
        settle().await;
        assert_eq!(
            hits(&second_ep).await,
            1,
            "a deleted webhook must stop receiving events"
        );
    }

    // --- the emission sites -----------------------------------------------
    //
    // Each mock matches the *exact* body it expects, so a hit proves both that
    // the site emits and that the event carried the right data.

    /// Echoes one variable plus the event kind, so a site wired to the wrong
    /// variant cannot pass.
    fn echo_template(variable: &str) -> String {
        format!(
            r#"{{"content":"{{{{{variable}}}}}","type":"{{{{NotificationType}}}}"}}"#
        )
    }

    fn echoed(content: &str, notification_type: NotificationType) -> String {
        format!(r#"{{"content":"{content}","type":"{notification_type}"}}"#)
    }

    async fn report_playback_start(
        server: &TestServer,
        h: &HeaderName,
        v: &HeaderValue,
        item_id: Uuid,
    ) {
        server
            .post("/sessions/playing")
            .add_header(h.clone(), v.clone())
            .json(&json!({
                "ItemId": item_id,
                "PlaySessionId": "emission-test",
                "PositionTicks": 1_500_000_000i64,
                "CanSeek": true,
                "IsPaused": false,
                "IsMuted": false,
                "PlayMethod": "DirectPlay",
            }))
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }

    async fn my_user_id(server: &TestServer, h: &HeaderName, v: &HeaderValue) -> Uuid {
        let me: serde_json::Value = server
            .get("/users/me")
            .add_header(h.clone(), v.clone())
            .await
            .json();
        Uuid::parse_str(
            me["Id"]
                .as_str()
                .expect("/users/me must carry an Id"),
        )
        .expect("the reported id must be a uuid")
    }

    #[tokio::test]
    async fn a_playback_start_reaches_a_configured_webhook() {
        let (server, guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let media = crate::integration_test::insert_test_source(&guard.0).await;

        let endpoint_server = MockServer::start_async().await;
        let endpoint = endpoint_server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/hook")
                    .body(echoed(&media.title, NotificationType::PlaybackStart));
                then.status(200);
            })
            .await;

        create(
            &server,
            &h,
            &v,
            &WebhookDto {
                notification_types: vec![NotificationType::PlaybackStart],
                template: echo_template("Name"),
                ..hook_dto("playback", &endpoint_server.url("/hook"))
            },
        )
        .await;

        report_playback_start(&server, &h, &v, media.id).await;

        eventually("the playback start to reach the webhook", async || {
            hits(&endpoint).await == 1
        })
        .await;
    }

    /// A stop report that records nothing must report nothing: the endpoint
    /// answers 204 to any authenticated client for any item id, so an event
    /// derived from the request rather than from what was written would let
    /// that client forge playback against the operator's endpoint.
    #[tokio::test]
    async fn a_stop_for_an_unknown_item_emits_nothing_and_still_answers_204() {
        let (server, guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let endpoint_server = MockServer::start_async().await;
        // Deliberately unconstrained: any delivery at all is a failure here.
        let forged = endpoint_server.mock(|when, then| {
            when.method(POST)
                .path("/forged");
            then.status(200);
        });
        let canary_ep = endpoint_server.mock(|when, then| {
            when.method(POST)
                .path("/canary");
            then.status(200);
        });

        create(
            &server,
            &h,
            &v,
            &WebhookDto {
                notification_types: vec![
                    NotificationType::PlaybackStop,
                    NotificationType::UserDataSaved,
                ],
                ..hook_dto("forgeable", &endpoint_server.url("/forged"))
            },
        )
        .await;
        create(
            &server,
            &h,
            &v,
            &hook_dto("canary", &endpoint_server.url("/canary")),
        )
        .await;

        server
            .post("/sessions/playing/stopped")
            .add_header(h.clone(), v.clone())
            .json(&json!({
                "ItemId": Uuid::from_u128(0xf0f0),
                "PlaySessionId": "never-started",
                "PositionTicks": 9_000_000_000i64,
                "CanSeek": true,
                "IsPaused": false,
                "IsMuted": false,
            }))
            .await
            .assert_status(StatusCode::NO_CONTENT);

        // The canary rides the same dispatcher: once it has seen an event
        // emitted *after* the stop, the stop has been fully dispatched.
        guard
            .0
            .webhooks
            .emit(generic_event());
        eventually("the dispatcher to drain past the stop", async || {
            hits(&canary_ep).await == 1
        })
        .await;
        settle().await;
        assert_eq!(
            hits(&forged).await,
            0,
            "a stop that recorded nothing must not manufacture playback events"
        );
    }

    /// Every other test here runs with a freshly widened mask, so a `reload`
    /// that computed an empty mask would pass the whole suite while permanently
    /// suppressing every guarded event on a real server.
    #[tokio::test]
    async fn a_reload_narrows_the_probe_to_the_subscribed_types() {
        let (server, guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let endpoint_server = MockServer::start_async().await;

        create(
            &server,
            &h,
            &v,
            &WebhookDto {
                notification_types: vec![NotificationType::PlaybackStart],
                ..hook_dto("starts only", &endpoint_server.url("/hook"))
            },
        )
        .await;

        guard
            .0
            .webhooks
            .emit(generic_event());
        eventually(
            "the dispatcher to reload and narrow the probe",
            async || {
                !guard
                    .0
                    .webhooks
                    .wants(NotificationType::ItemAdded)
            },
        )
        .await;

        assert!(
            guard
                .0
                .webhooks
                .wants(NotificationType::PlaybackStart),
            "the one subscribed type must survive the narrowing"
        );
    }

    /// The row is gone by the time the payload is built, so this only works
    /// because it is captured before the DELETE.
    #[tokio::test]
    async fn an_item_deletion_carries_the_deleted_items_data() {
        let (server, guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let media = crate::integration_test::insert_test_source(&guard.0).await;

        let endpoint_server = MockServer::start_async().await;
        let endpoint = endpoint_server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/hook")
                    .body(echoed(&media.title, NotificationType::ItemDeleted));
                then.status(200);
            })
            .await;

        create(
            &server,
            &h,
            &v,
            &WebhookDto {
                notification_types: vec![NotificationType::ItemDeleted],
                template: echo_template("Name"),
                ..hook_dto("deletions", &endpoint_server.url("/hook"))
            },
        )
        .await;

        server
            .delete(&format!("/items/{}", media.id))
            .add_header(h.clone(), v.clone())
            .await
            .assert_status(StatusCode::NO_CONTENT);

        assert!(
            db::Media::get_by_id(
                &guard
                    .0
                    .db,
                &media.id
            )
            .await
            .expect("the lookup must succeed")
            .is_none(),
            "the row must really be gone, or this test proves nothing"
        );

        eventually("the deletion to reach the webhook", async || {
            hits(&endpoint).await == 1
        })
        .await;
    }

    /// Emitting must not change the 401 a client sees, and a successful login
    /// must not emit at all.
    #[tokio::test]
    async fn an_authentication_failure_emits_without_changing_the_401() {
        let (server, _guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);

        let bad_login = async || {
            server
                .post("/users/authenticatebyname")
                .add_header(
                    http::header::AUTHORIZATION,
                    HeaderValue::from_static(AUTH_HEADER),
                )
                .json(&json!({ "Username": "test", "Pw": "wrong" }))
                .expect_failure()
                .await
        };

        let before = bad_login().await;
        before.assert_status(StatusCode::UNAUTHORIZED);
        let before_body = before.text();

        let endpoint_server = MockServer::start_async().await;
        let endpoint = endpoint_server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/hook")
                    .body(echoed("test", NotificationType::AuthenticationFailure));
                then.status(200);
            })
            .await;
        create(
            &server,
            &h,
            &v,
            &WebhookDto {
                notification_types: vec![NotificationType::AuthenticationFailure],
                template: echo_template("NotificationUsername"),
                ..hook_dto("failures", &endpoint_server.url("/hook"))
            },
        )
        .await;

        let after = bad_login().await;
        after.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(
            after.text(),
            before_body,
            "emitting must not change the refusal a client sees"
        );

        eventually("the failed login to reach the webhook", async || {
            hits(&endpoint).await == 1
        })
        .await;

        // The credential check is the trigger, not the endpoint.
        server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER),
            )
            .json(&json!({ "Username": "test", "Pw": "test" }))
            .await
            .assert_status_ok();
        settle().await;
        assert_eq!(
            hits(&endpoint).await,
            1,
            "a successful login must not emit AuthenticationFailure"
        );
    }

    // --- the filters, against the event the server actually builds ----------

    /// One `PlaybackStart`, three hooks, one delivery.
    ///
    /// `WebhookService::matches` is unit-tested against hand-built events, which
    /// cannot see what the *emission site* puts in one. The subscribed hook here
    /// filters on the id `/users/me` reports, and echoes
    /// `{{NotificationUsername}}` — which comes from the `&db::User →
    /// UserEventData` conversion that no other test pins end to end.
    ///
    /// That same hook is the canary for the two zero assertions: the dispatcher
    /// picks all three targets in a single pass over the cached hook set.
    #[tokio::test]
    async fn a_playback_start_reaches_only_the_hooks_whose_filters_accept_it() {
        let (server, guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let media = crate::integration_test::insert_test_source(&guard.0).await;
        let me = my_user_id(&server, &h, &v).await;

        let endpoint_server = MockServer::start_async().await;
        let subscribed = endpoint_server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/subscribed")
                    // "test" is the user `authenticated_server` seeds and logs in.
                    .body(echoed("test", NotificationType::PlaybackStart));
                then.status(200);
            })
            .await;
        // Deliberately unconstrained: any request at all is a failure.
        let mut reject = |path: &'static str| {
            endpoint_server.mock(|when, then| {
                when.method(POST)
                    .path(path);
                then.status(200);
            })
        };
        let wrong_type = reject("/wrong-type");
        let wrong_user = reject("/wrong-user");

        create(
            &server,
            &h,
            &v,
            &WebhookDto {
                notification_types: vec![NotificationType::PlaybackStart],
                user_filter: vec![me],
                template: echo_template("NotificationUsername"),
                ..hook_dto("mine", &endpoint_server.url("/subscribed"))
            },
        )
        .await;
        create(
            &server,
            &h,
            &v,
            &WebhookDto {
                notification_types: vec![NotificationType::ItemDeleted],
                ..hook_dto("deletions only", &endpoint_server.url("/wrong-type"))
            },
        )
        .await;
        create(
            &server,
            &h,
            &v,
            &WebhookDto {
                notification_types: vec![NotificationType::PlaybackStart],
                user_filter: vec![Uuid::from_u128(0xf0f0)],
                ..hook_dto("someone else", &endpoint_server.url("/wrong-user"))
            },
        )
        .await;

        report_playback_start(&server, &h, &v, media.id).await;

        eventually("the subscribed hook to be delivered to", async || {
            hits(&subscribed).await == 1
        })
        .await;
        settle().await;
        assert_eq!(
            hits(&wrong_type).await,
            0,
            "a hook subscribed to another type must not receive this event"
        );
        assert_eq!(
            hits(&wrong_user).await,
            0,
            "a hook filtered on another user must not receive this event"
        );
    }

    // --- the enabled switch, end to end -------------------------------------

    /// Neither half of this is proven by the parts: a `reload` that read
    /// `get_all` instead of `get_enabled` would keep every other test green
    /// while making the operator's kill switch do nothing until a restart.
    ///
    /// The canary is created once and never touched again; its second hit is
    /// what proves the post-disable event was dispatched.
    #[tokio::test]
    async fn disabling_a_hook_over_http_stops_its_deliveries() {
        let (server, guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let endpoint_server = MockServer::start_async().await;

        let mut endpoint = |path: &'static str| {
            endpoint_server.mock(|when, then| {
                when.method(POST)
                    .path(path);
                then.status(200);
            })
        };
        let canary_ep = endpoint("/canary");
        let target_ep = endpoint("/target");

        create(
            &server,
            &h,
            &v,
            &hook_dto("canary", &endpoint_server.url("/canary")),
        )
        .await;
        let target = create(
            &server,
            &h,
            &v,
            &hook_dto("target", &endpoint_server.url("/target")),
        )
        .await;

        guard
            .0
            .webhooks
            .emit(generic_event());
        eventually("the enabled hook to be delivered to", async || {
            hits(&canary_ep).await == 1 && hits(&target_ep).await == 1
        })
        .await;

        let disabled: WebhookDto = server
            .post(&format!("/remux/webhooks/{}", target.id))
            .add_header(h.clone(), v.clone())
            .json(&WebhookDto {
                enabled: false,
                ..hook_dto("target", &endpoint_server.url("/target"))
            })
            .await
            .json();
        assert!(!disabled.enabled, "the write must have taken effect");

        guard
            .0
            .webhooks
            .emit(generic_event());
        eventually("the canary to see the second event", async || {
            hits(&canary_ep).await == 2
        })
        .await;
        settle().await;
        assert_eq!(
            hits(&target_ep).await,
            1,
            "a disabled webhook must stop receiving events"
        );
    }

    // --- the Discord destination, end to end --------------------------------

    /// The destination's settings are template *variables*, so this only works
    /// if the whole chain holds: the `Discord` variant survives the DB's JSON
    /// column, the reload hands it to `with_hook_fields`, the overlay lands
    /// under the plugin's key spellings, and the sender posts it as JSON. Every
    /// link is unit-tested; nothing composed them.
    #[tokio::test]
    async fn a_discord_hook_posts_the_rendered_discord_envelope() {
        let (server, guard, token) = authenticated_server().await;
        let (h, v) = auth(&token);
        let media = crate::integration_test::insert_test_source(&guard.0).await;

        // `#AA5CC3` as the integer Discord wants, spelled out rather than
        // computed so the plugin's off-by-one hex truncation cannot creep back.
        let expected = format!(
            r#"{{"username":"remux","avatar_url":"https://example.test/a.png","content":"@everyone","embeds":[{{"color":11164867,"description":"{}"}}]}}"#,
            media.title
        );
        let endpoint_server = MockServer::start_async().await;
        let endpoint = endpoint_server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/api/webhooks/1/token")
                    .header("content-type", "application/json; charset=utf-8")
                    .body(&expected);
                then.status(204);
            })
            .await;

        create(
            &server,
            &h,
            &v,
            &WebhookDto {
                notification_types: vec![NotificationType::PlaybackStart],
                destination: WebhookDestination::Discord {
                    avatar_url: Some("https://example.test/a.png".into()),
                    bot_username: Some("remux".into()),
                    embed_color: Some("#AA5CC3".into()),
                    mention_type: DiscordMentionType::Everyone,
                },
                template: r#"{"username":"{{BotUsername}}","avatar_url":"{{AvatarUrl}}","content":"{{MentionType}}","embeds":[{"color":{{EmbedColor}},"description":"{{Name}}"}]}"#.into(),
                ..hook_dto(
                    "discord",
                    &endpoint_server.url("/api/webhooks/1/token"),
                )
            },
        )
        .await;

        report_playback_start(&server, &h, &v, media.id).await;

        eventually("the discord envelope to reach the endpoint", async || {
            hits(&endpoint).await == 1
        })
        .await;
    }
}
