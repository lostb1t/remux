//! Admin CRUD over outgoing webhooks, plus the synchronous "test this webhook"
//! endpoint the dashboard uses for immediate feedback.
//!
//! Two invariants hold across every handler here.
//!
//! **Every route is admin-only.** A webhook URL is a credential — Discord's is
//! `https://discord.com/api/webhooks/{id}/{token}` and that token is the entire
//! authentication — so read access is as sensitive as write access. `session:
//! auth::AdminSession` in the signature is the whole mechanism; there is no
//! path into this module without it.
//!
//! **Every mutation ends in `state.ctx.webhooks.invalidate()`.** The dispatcher
//! caches the enabled hook set and reloads only when that flag is set, so a
//! write that skips the call returns a perfect 200 and then silently does
//! nothing until the process restarts.

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
/// Parse, don't validate: the value cannot be constructed from anything but an
/// absolute `http(s)` URL with a host, so nothing downstream — the DB row, the
/// dispatcher's cached snapshot, the delivery task — has to re-check. The
/// scheme restriction is not cosmetic: `Url::parse` cheerfully accepts
/// `file:///etc/shadow` and `javascript:alert(1)`, and neither belongs anywhere
/// near the delivery path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookUrl(Url);

impl WebhookUrl {
    /// The canonical serialization — this, not the operator's raw string, is
    /// what gets stored.
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

/// The stored webhook, or a 404. Every by-id route starts here so a missing row
/// is a 404 rather than a 500 out of the repository's re-read.
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
    let payload = with_parsed_url(payload)?;
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
    let payload = with_parsed_url(payload)?;
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
/// carrying `success: false`: the *request* worked, the *test* did not, and the
/// dashboard needs the difference.
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
        NotificationType, WebhookDestination, WebhookItemTypes, WebhookKeyValue,
    };
    use serde_json::json;
    use std::time::{Duration, Instant};

    /// A body that is valid JSON and echoes exactly one variable, so a received
    /// request pins both the template output and the variable dictionary.
    const TEMPLATE: &str = r#"{"content":"{{Name}}"}"#;

    fn auth(token: &str) -> (HeaderName, HeaderValue) {
        (
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&auth_header_with_token(token)).unwrap(),
        )
    }

    /// A fully populated create payload. `id` is deliberately non-nil so the
    /// round-trip proves the server assigns its own.
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
    /// not. The dispatcher spawns deliveries, so "the canary was hit" only
    /// proves the event was *dispatched*, not that a stray socket has settled.
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

    /// The rejection travels back to the browser and into logs, so it must not
    /// carry the URL it is rejecting.
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

        // The update is persisted, not just echoed.
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

    /// The URL is parsed, not trusted: a hook whose URL cannot be posted to is
    /// a hook that fails silently in a background task forever after.
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

    /// A webhook URL embeds a credential (Discord's is
    /// `.../webhooks/{id}/{token}`), so a non-admin must not be able to read
    /// one — not through the list, not through a by-id read.
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

    /// The test endpoint bypasses the broadcast channel entirely: the hook here
    /// is disabled and subscribes to nothing, so the dispatcher would never
    /// deliver to it. It must still be tested, synchronously, and the endpoint's
    /// answer must come back to the caller.
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

    /// A failing endpoint is a failed *test*, not a failed request: the
    /// dashboard needs the status to show it. And it is one attempt — the retry
    /// policy belongs to background delivery, not to an operator waiting on an
    /// answer.
    ///
    /// The remote's **response body** must not come back. The URL is
    /// admin-controlled and unrestricted by host, so echoing what the endpoint
    /// said would make this route a read primitive against anything the server
    /// can reach; this asserts on the raw HTTP response, not just the parsed
    /// field, so no route out of the handler is missed.
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

    /// An unreachable endpoint must come back as a failed test rather than
    /// hanging the handler or leaking the URL path into the response.
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

    // --- dispatcher cache invalidation ------------------------------------

    /// `invalidate()` is how a saved webhook reaches the *running* dispatcher:
    /// it caches the enabled hook set and reloads only when that flag is set.
    /// A create, update or delete that forgets the call looks perfect over HTTP
    /// and silently does nothing until the process restarts — so this drives
    /// the real cycle (write over HTTP, emit an event, watch the socket).
    ///
    /// The canary hook is never touched after its creation. Its hit count is
    /// the synchronisation point: once it has seen event N, the dispatcher has
    /// finished dispatching event N, which is what makes the negative
    /// assertions below meaningful rather than a race.
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
    // These drive real HTTP endpoints and watch a real socket. Each mock
    // matches the *exact* body it expects, so a hit proves both that the site
    // emits and that the event carried the right data — a wrong payload leaves
    // the mock at zero hits and fails the wait.

    /// The one variable every template below echoes, plus the event kind, so a
    /// site wired to the wrong variant cannot pass.
    fn echo_template(variable: &str) -> String {
        format!(
            r#"{{"content":"{{{{{variable}}}}}","type":"{{{{NotificationType}}}}"}}"#
        )
    }

    fn echoed(content: &str, notification_type: NotificationType) -> String {
        format!(r#"{{"content":"{content}","type":"{notification_type}"}}"#)
    }

    /// `POST /sessions/playing` reaches a hook subscribed to `PlaybackStart`,
    /// carrying the item that is being played.
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

        server
            .post("/sessions/playing")
            .add_header(h.clone(), v.clone())
            .json(&json!({
                "ItemId": media.id,
                "PlaySessionId": "emission-test",
                "PositionTicks": 1_500_000_000i64,
                "CanSeek": true,
                "IsPaused": false,
                "IsMuted": false,
                "PlayMethod": "DirectPlay",
            }))
            .await
            .assert_status(StatusCode::NO_CONTENT);

        eventually("the playback start to reach the webhook", async || {
            hits(&endpoint).await == 1
        })
        .await;
    }

    /// `DELETE /items/{id}` reaches a hook subscribed to `ItemDeleted` with the
    /// deleted item's own data — which only works because the row is captured
    /// before the DELETE. The row is gone by the time the payload is built, so
    /// anything that re-read it would render an empty name.
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

    /// A failed login emits `AuthenticationFailure` — and answers with exactly
    /// the same 401 it answered before any webhook existed. A successful login
    /// must not emit it.
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

        // Baseline: the refusal as it is with nothing listening.
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

        // The credential check is the trigger, not the endpoint: a login that
        // succeeds must not report a failure.
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
}
