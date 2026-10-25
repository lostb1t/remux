//! OpenID Connect sign-in endpoints. The paths mirror the Jellyfin SSO
//! plugin (`/sso/OID/start/{provider}`, `/sso/OID/redirect/{provider}`) so
//! links and provider registrations made for it keep working.
//!
//! Flow: `start` redirects to the provider → the provider redirects back to
//! `redirect`, which resolves the local user and serves a small page holding
//! a one-time code → that page POSTs the code to `auth` with the browser's
//! own device headers, stores the returned credentials the way jellyfin-web
//! and the dashboard expect, and navigates to the original destination.

use axum::{
    Json,
    extract::{Path, State},
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::Query;
use http::{HeaderMap, StatusCode, header};
use remux_macros::{get, post, query};
use remux_sdks::remux::SsoProvider;
use serde::Deserialize;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    AppState, OptionExt, ResultExt,
    api::users::build_auth_response,
    db::{self, SsoLink, User, auth},
    oidc::{self, Identity, OidcProvider, PendingLogin, ReturnPath},
    signals::{Event, UserUpdatedInfo},
};
use axum_anyhow::ApiResult as Result;

fn pending_key(state: &str) -> String {
    format!("oidc:state:{state}")
}

fn login_code_key(code: &str) -> String {
    format!("oidc:code:{code}")
}

/// Proof that the provider vouched for `user_id`, redeemable once.
struct LoginCode {
    user_id: Uuid,
}

/// A failed sign-in, shown as a page: the browser is mid-redirect, so a JSON
/// error would be a dead end.
#[derive(Debug, thiserror::Error)]
enum SsoError {
    #[error("This sign-in provider is not configured.")]
    UnknownProvider,
    #[error("This sign-in attempt has expired. Please start again.")]
    Expired,
    #[error("Invalid return address: {0}")]
    InvalidReturnPath(String),
    #[error("The identity provider did not complete the sign-in: {0}")]
    Provider(String),
    #[error("Your account is not allowed to use this server.")]
    NotAllowed,
    #[error(
        "A local account named \"{0}\" already exists and is not linked to this provider."
    )]
    UsernameTaken(String),
    #[error("No account exists for you on this server yet. Ask an administrator.")]
    NoAccount,
    #[error("Sign-in failed. The server log has the details.")]
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for SsoError {
    fn from(e: anyhow::Error) -> Self {
        SsoError::Internal(e)
    }
}

impl IntoResponse for SsoError {
    fn into_response(self) -> Response {
        let status = match &self {
            SsoError::UnknownProvider => StatusCode::NOT_FOUND,
            SsoError::Expired
            | SsoError::InvalidReturnPath(_)
            | SsoError::Provider(_) => StatusCode::BAD_REQUEST,
            SsoError::NotAllowed | SsoError::UsernameTaken(_) | SsoError::NoAccount => {
                StatusCode::FORBIDDEN
            }
            SsoError::Internal(e) => {
                error!(err = ?e, "OIDC sign-in failed");
                StatusCode::BAD_GATEWAY
            }
        };
        let body = format!(
            // Both /sso/oid/start/x and /sso/oid/redirect/x sit three levels
            // below the server root, wherever a reverse proxy mounts it.
            r#"<p>{}</p><p><a href="../../../">Back to sign-in</a></p>"#,
            html_escape(&self.to_string())
        );
        (
            status,
            no_store_headers(),
            Html(page("Sign-in failed", &body)),
        )
            .into_response()
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// The callback URL carries `code`/`state`; keep it out of caches and
/// `Referer` headers.
fn no_store_headers() -> [(header::HeaderName, &'static str); 2] {
    [
        (header::CACHE_CONTROL, "no-store"),
        (header::REFERRER_POLICY, "no-referrer"),
    ]
}

fn page(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{title}</title><style>body{{font-family:system-ui,sans-serif;background:#101010;color:#eee;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0}}main{{max-width:28rem;padding:1.5rem;text-align:center}}a{{color:#00a4dc}}</style></head><body><main>{body}</main></body></html>"#
    )
}

fn provider_for<'a>(
    state: &'a AppState,
    name: &str,
) -> std::result::Result<&'a OidcProvider, SsoError> {
    state
        .ctx
        .oidc
        .as_deref()
        .filter(|p| {
            p.name
                .as_str()
                .eq_ignore_ascii_case(name)
        })
        .ok_or(SsoError::UnknownProvider)
}

/// Lists configured providers so login pages can offer "Sign in with …".
#[get("/remux/sso/providers")]
pub async fn sso_providers(State(state): State<AppState>) -> Json<Vec<SsoProvider>> {
    Json(
        state
            .ctx
            .oidc
            .iter()
            .map(|p| SsoProvider {
                name: p
                    .name
                    .as_str()
                    .to_string(),
                display_name: p
                    .display_name
                    .clone(),
            })
            .collect(),
    )
}

#[query]
#[derive(Default)]
struct StartQuery {
    redirect: Option<String>,
}

#[get("/sso/oid/start/{provider}")]
pub async fn sso_start(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    Query(query): Query<StartQuery>,
) -> std::result::Result<Response, SsoError> {
    let provider = provider_for(&state, &provider)?;
    let return_to = match query
        .redirect
        .as_deref()
    {
        Some(path) => path
            .parse::<ReturnPath>()
            .map_err(|e| SsoError::InvalidReturnPath(e.to_string()))?,
        None => ReturnPath::default(),
    };
    let redirect_uri = provider.redirect_uri(&headers)?;
    let login = provider
        .begin_login(redirect_uri, return_to)
        .await?;

    state
        .ctx
        .store
        .insert(
            pending_key(&login.state),
            login.pending,
            oidc::PENDING_LOGIN_TTL,
        );
    Ok(Redirect::to(
        login
            .authorization_url
            .as_str(),
    )
    .into_response())
}

#[query]
#[derive(Default)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[get("/sso/oid/redirect/{provider}")]
pub async fn sso_redirect(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(query): Query<CallbackQuery>,
) -> std::result::Result<Response, SsoError> {
    let provider = provider_for(&state, &provider)?;

    // Take the pending login first so a state can't be replayed, even when
    // the provider reports an error.
    let pending = query
        .state
        .as_deref()
        .and_then(|s| {
            let key = pending_key(s);
            let pending = state
                .ctx
                .store
                .get::<PendingLogin>(&key);
            state
                .ctx
                .store
                .delete(key);
            pending
        })
        .filter(|p| p.provider == provider.name)
        .ok_or(SsoError::Expired)?;

    if let Some(err) = query.error {
        return Err(SsoError::Provider(
            query
                .error_description
                .unwrap_or(err),
        ));
    }
    let code = query
        .code
        .ok_or_else(|| SsoError::Provider("no authorization code".into()))?;

    let identity = provider
        .complete_login(&code, &pending)
        .await?;
    let user = resolve_user(&state, provider, &identity).await?;
    info!(
        user = %user.username,
        provider = provider.name.as_str(),
        "OIDC sign-in"
    );

    let login_code = oidc::random_token();
    state
        .ctx
        .store
        .insert(
            login_code_key(&login_code),
            LoginCode { user_id: user.id },
            oidc::LOGIN_CODE_TTL,
        );
    Ok((
        no_store_headers(),
        Html(completion_page(&login_code, &pending.return_to)),
    )
        .into_response())
}

/// Find the local user for `identity`, linking or creating one per config,
/// and apply group-driven admin status.
async fn resolve_user(
    state: &AppState,
    provider: &OidcProvider,
    identity: &Identity,
) -> std::result::Result<User, SsoError> {
    if !provider.is_allowed(identity) {
        return Err(SsoError::NotAllowed);
    }
    let db = &state
        .ctx
        .db;
    let provider_name = provider
        .name
        .as_str();

    let mut changed = false;
    let mut user =
        match SsoLink::find_user(db, provider_name, &identity.subject).await? {
            Some(user) => user,
            None => {
                changed = true;
                let user = match User::get_by_username(
                    db,
                    identity
                        .username
                        .as_str(),
                )
                .await?
                {
                    Some(existing) if provider.link_existing_users => existing,
                    Some(existing) => {
                        return Err(SsoError::UsernameTaken(existing.username));
                    }
                    None if provider.auto_create_users => {
                        // SSO-only account: an unguessable password nobody knows.
                        // An admin can set a real one later.
                        let mut user = User::new_with_password(
                            String::new(),
                            identity
                                .username
                                .to_string(),
                            &oidc::random_token(),
                            None,
                        )?;
                        user.is_admin = provider
                            .admin_flag(identity)
                            .unwrap_or(false);
                        user.save(db)
                            .await?;
                        user
                    }
                    None => return Err(SsoError::NoAccount),
                };
                SsoLink::link(db, provider_name, &identity.subject, user.id).await?;
                user
            }
        };

    if let Some(is_admin) = provider.admin_flag(identity) {
        if user.is_admin != is_admin {
            warn!(
                user = %user.username,
                is_admin,
                "OIDC groups changed admin status"
            );
            user.is_admin = is_admin;
            user.save(db)
                .await?;
            changed = true;
        }
    }
    if changed {
        state
            .ctx
            .signals
            .emit(Event::UserUpdated(UserUpdatedInfo { user_id: user.id }));
    }
    Ok(user)
}

/// Runs in the browser that finished the provider redirect. It sends the
/// client's own device id, so the session lands on the same device row that
/// jellyfin-web (`_deviceId2`, a bare string) or the dashboard
/// (`remux_device_id`, JSON-encoded by gloo-storage) uses afterwards.
const COMPLETION_SCRIPT: &str = r#"
(function () {
  var cfg = __CONFIG__;
  var status = document.getElementById('status');
  var path = window.location.pathname;
  var cut = path.toLowerCase().indexOf('/sso/');
  var base = window.location.origin + (cut > 0 ? path.slice(0, cut) : '');
  var admin = cfg.returnTo.toLowerCase().indexOf('/admin') === 0;

  function deviceId() {
    var key = admin ? 'remux_device_id' : '_deviceId2';
    var id = localStorage.getItem(key);
    if (id && admin) { try { id = JSON.parse(id); } catch (e) { id = null; } }
    if (!id) {
      if (admin) {
        id = 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, function (c) {
          var r = Math.random() * 16 | 0;
          return (c === 'x' ? r : (r & 3 | 8)).toString(16);
        });
        localStorage.setItem(key, JSON.stringify(id));
      } else {
        id = btoa([navigator.userAgent, new Date().getTime()].join('|')).replace(/=/g, '1');
        localStorage.setItem(key, id);
      }
    }
    return id;
  }

  function browser() {
    var ua = navigator.userAgent;
    if (/Edg\//.test(ua)) return 'Edge';
    if (/Firefox\//.test(ua)) return 'Firefox';
    if (/Chrome\//.test(ua)) return 'Chrome';
    if (/Safari\//.test(ua)) return 'Safari';
    return 'Browser';
  }

  function q(v) { return '"' + encodeURIComponent(v) + '"'; }

  var auth = 'MediaBrowser Client=' + q(admin ? 'Remux Dashboard' : 'Jellyfin Web') +
    ', Device=' + q(admin ? 'Browser' : browser()) +
    ', DeviceId=' + q(deviceId()) +
    ', Version=' + q(admin ? '1.0.0' : 'SSO');

  fetch(base + '/sso/oid/auth', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'Authorization': auth },
    body: JSON.stringify({ Code: cfg.code })
  }).then(function (r) {
    if (!r.ok) throw new Error('server returned ' + r.status);
    return r.json();
  }).then(function (result) {
    var creds = null;
    try { creds = JSON.parse(localStorage.getItem('jellyfin_credentials')); } catch (e) {}
    if (!creds || !Array.isArray(creds.Servers)) creds = { Servers: [] };
    creds.Servers = creds.Servers.filter(function (s) { return s && s.Id !== result.ServerId; });
    creds.Servers.unshift({
      Id: result.ServerId,
      Name: 'Remux',
      ManualAddress: base,
      LocalAddress: base,
      AccessToken: result.AccessToken,
      UserId: result.User.Id,
      DateLastAccessed: Date.now(),
      LastConnectionMode: 2
    });
    localStorage.setItem('jellyfin_credentials', JSON.stringify(creds));
    localStorage.setItem('enableAutoLogin', 'true');
    window.location.replace(base + cfg.returnTo);
  }).catch(function (e) {
    status.textContent = 'Sign-in failed: ' + e.message;
  });
})();
"#;

fn completion_page(code: &str, return_to: &ReturnPath) -> String {
    // `<` escaped so nothing in the values can close the script element.
    let config = serde_json::json!({ "code": code, "returnTo": return_to.as_str() })
        .to_string()
        .replace('<', "\\u003c");
    page(
        "Signing in…",
        &format!(
            r#"<p id="status">Signing in…</p><script>{}</script>"#,
            COMPLETION_SCRIPT.replace("__CONFIG__", &config)
        ),
    )
}

#[derive(Deserialize)]
struct SsoAuthRequest {
    #[serde(alias = "code")]
    #[serde(rename = "Code")]
    code: String,
}

/// Trade a one-time code from the completion page for a session, exactly
/// like `/users/authenticatebyname` would return one.
#[post("/sso/oid/auth")]
pub async fn sso_auth(
    State(state): State<AppState>,
    auth_header: auth::JellyfinAuthHeader,
    Json(body): Json<SsoAuthRequest>,
) -> Result<impl IntoResponse> {
    let key = login_code_key(&body.code);
    let login = state
        .ctx
        .store
        .get::<LoginCode>(&key)
        .context_unauthorized("sign-in code is invalid or expired")?;
    state
        .ctx
        .store
        .delete(key);

    let user = User::get_by_id(
        &state
            .ctx
            .db,
        &login.user_id,
    )
    .await?
    .context_unauthorized("user not found")?;
    let device = auth::Device::new_from_header(auth_header, &user)?;
    device
        .save(
            &state
                .ctx
                .db,
        )
        .await?;

    Ok(build_auth_response(
        &state
            .ctx
            .config
            .data_dir,
        device,
        user,
    ))
}

#[cfg(test)]
mod tests {
    use axum_test::{TestResponse, TestServer};
    use base64::{
        Engine,
        engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    };
    use http::header::HeaderValue;
    use httpmock::{
        Method::{GET, POST},
        MockServer,
    };
    use serde_json::{Value, json};
    use url::Url;

    use crate::{
        Config,
        integration_test::{
            AUTH_HEADER, TestGuard, auth_header_with_token, new_test_server,
            new_test_server_with_config,
        },
    };

    const REDIRECT_URI: &str = "http://remux.test/sso/oid/redirect/authentik";
    const CLIENT_ID: &str = "remux";
    // Placeholder for the mock provider only.
    const CLIENT_SECRET: &str = "test-client-secret";

    /// Signature is never checked (see `oidc` module docs), so any will do.
    fn jwt(claims: Value) -> String {
        format!(
            "{}.{}.sig",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        )
    }

    async fn mock_idp() -> MockServer {
        let idp = MockServer::start_async().await;
        let issuer = idp.base_url();
        idp.mock_async(|when, then| {
            when.method(GET)
                .path("/.well-known/openid-configuration");
            then.status(200)
                .json_body(json!({
                    "issuer": issuer,
                    "authorization_endpoint": format!("{issuer}/authorize"),
                    "token_endpoint": format!("{issuer}/token"),
                    "userinfo_endpoint": format!("{issuer}/userinfo"),
                }));
        })
        .await;
        idp
    }

    async fn server_with_idp(idp: &MockServer) -> (TestServer, TestGuard) {
        new_test_server_with_config(Config {
            database_url: Some("sqlite::memory:".into()),
            torrent_http_port: None,
            torrent_peer_port: None,
            disable_dht: true,
            disable_startup_tasks: true,
            oidc_issuer_url: Some(idp.base_url()),
            oidc_client_id: Some(CLIENT_ID.into()),
            oidc_client_secret: Some(
                CLIENT_SECRET
                    .to_string()
                    .into(),
            ),
            oidc_provider_name: "authentik".into(),
            oidc_display_name: "Authentik".into(),
            oidc_admin_groups: Some("remux-admins".into()),
            oidc_redirect_url: Some(REDIRECT_URI.into()),
            ..Default::default()
        })
        .await
        .unwrap()
    }

    /// Start a sign-in and play the provider: returns the callback response,
    /// which must be a success when `expect_ok` and an error page otherwise.
    async fn provider_callback(
        server: &TestServer,
        idp: &MockServer,
        code: &str,
        sub: &str,
        username: &str,
        groups: &[&str],
        expect_ok: bool,
    ) -> TestResponse {
        let start = server
            .get("/sso/oid/start/authentik")
            .add_query_param("redirect", "/admin/")
            .expect_failure()
            .await;
        start.assert_status(http::StatusCode::SEE_OTHER);
        let location = Url::parse(
            &start
                .header("location")
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let param = |name: &str| {
            location
                .query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
                .unwrap_or_else(|| panic!("authorization URL lacks {name}"))
        };
        assert_eq!(location.path(), "/authorize");
        assert_eq!(param("client_id"), CLIENT_ID);
        assert_eq!(param("redirect_uri"), REDIRECT_URI);
        assert_eq!(param("code_challenge_method"), "S256");
        assert!(param("scope").starts_with("openid"));

        let issuer = idp.base_url();
        let access_token = format!("at-{code}");
        idp.mock_async(|when, then| {
            when.method(POST)
                .path("/token")
                // client_secret_basic, built from the test placeholders.
                .header(
                    "authorization",
                    format!(
                        "Basic {}",
                        STANDARD.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}"))
                    ),
                )
                .body_contains(format!("code={code}"))
                .body_contains("code_verifier=");
            then.status(200)
                .json_body(json!({
                    "access_token": access_token,
                    "token_type": "Bearer",
                    "id_token": jwt(json!({
                        "iss": issuer,
                        "sub": sub,
                        "aud": CLIENT_ID,
                        "exp": chrono::Utc::now().timestamp() + 300,
                        "nonce": param("nonce"),
                    })),
                }));
        })
        .await;
        idp.mock_async(|when, then| {
            when.method(GET)
                .path("/userinfo")
                .header("authorization", format!("Bearer {access_token}"));
            then.status(200)
                .json_body(json!({
                    "sub": sub,
                    "preferred_username": username,
                    "groups": groups,
                }));
        })
        .await;

        let callback = server
            .get("/sso/oid/redirect/authentik")
            .add_query_param("code", code)
            .add_query_param("state", param("state"));
        if expect_ok {
            callback
                .expect_success()
                .await
        } else {
            callback
                .expect_failure()
                .await
        }
    }

    /// The `{code, returnTo}` object embedded in the completion page.
    fn completion_config(page: &str) -> Value {
        let start = page
            .find("var cfg = ")
            .expect("completion page has no config")
            + "var cfg = ".len();
        let end = start
            + page[start..]
                .find(';')
                .unwrap();
        serde_json::from_str(&page[start..end]).unwrap()
    }

    async fn redeem(server: &TestServer, code: &str, expect_ok: bool) -> TestResponse {
        let request = server
            .post("/sso/oid/auth")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER),
            )
            .json(&json!({ "Code": code }));
        if expect_ok {
            request
                .expect_success()
                .await
        } else {
            request
                .expect_failure()
                .await
        }
    }

    async fn sign_in(
        server: &TestServer,
        idp: &MockServer,
        code: &str,
        sub: &str,
        username: &str,
        groups: &[&str],
    ) -> Value {
        let callback =
            provider_callback(server, idp, code, sub, username, groups, true).await;
        callback.assert_status_ok();
        let cfg = completion_config(&callback.text());
        assert_eq!(cfg["returnTo"], "/admin/");
        let auth = redeem(
            server,
            cfg["code"]
                .as_str()
                .unwrap(),
            true,
        )
        .await;
        auth.assert_status_ok();
        auth.json()
    }

    async fn me(server: &TestServer, token: &str) -> Value {
        server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(token)).unwrap(),
            )
            .await
            .json()
    }

    #[tokio::test]
    async fn sign_in_creates_a_user_and_syncs_admin_from_groups() {
        let idp = mock_idp().await;
        let (server, _guard) = server_with_idp(&idp).await;

        let first =
            sign_in(&server, &idp, "c1", "sub-alice", "alice", &["remux-admins"]).await;
        let token = first["AccessToken"]
            .as_str()
            .unwrap();
        let user = me(&server, token).await;
        assert_eq!(user["Name"], "alice");
        assert_eq!(user["Policy"]["IsAdministrator"], true);

        // Same subject, admin group gone: same account, admin revoked.
        let second =
            sign_in(&server, &idp, "c2", "sub-alice", "alice", &["users"]).await;
        assert_eq!(second["User"]["Id"], first["User"]["Id"]);
        let user = me(
            &server,
            second["AccessToken"]
                .as_str()
                .unwrap(),
        )
        .await;
        assert_eq!(user["Policy"]["IsAdministrator"], false);
    }

    #[tokio::test]
    async fn state_and_login_codes_are_single_use() {
        let idp = mock_idp().await;
        let (server, _guard) = server_with_idp(&idp).await;

        let callback =
            provider_callback(&server, &idp, "c1", "sub-bob", "bob", &[], true).await;
        callback.assert_status_ok();
        let code = completion_config(&callback.text())["code"]
            .as_str()
            .unwrap()
            .to_string();
        redeem(&server, &code, true)
            .await
            .assert_status_ok();
        redeem(&server, &code, false)
            .await
            .assert_status(http::StatusCode::UNAUTHORIZED);

        let replay = server
            .get("/sso/oid/redirect/authentik")
            .add_query_param("code", "c1")
            .add_query_param("state", "not-a-real-state")
            .expect_failure()
            .await;
        replay.assert_status(http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn existing_local_username_is_not_taken_over_by_default() {
        let idp = mock_idp().await;
        let (server, _guard) = server_with_idp(&idp).await;

        // "test" is the admin seeded by the test harness.
        let callback =
            provider_callback(&server, &idp, "c1", "sub-mallory", "test", &[], false)
                .await;
        callback.assert_status(http::StatusCode::FORBIDDEN);
        assert!(
            callback
                .text()
                .contains("already exists")
        );
    }

    #[tokio::test]
    async fn start_rejects_offsite_return_paths() {
        let idp = mock_idp().await;
        let (server, _guard) = server_with_idp(&idp).await;
        server
            .get("/sso/oid/start/authentik")
            .add_query_param("redirect", "https://evil.example/")
            .expect_failure()
            .await
            .assert_status(http::StatusCode::BAD_REQUEST);
        server
            .get("/sso/oid/start/other")
            .expect_failure()
            .await
            .assert_status(http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn providers_are_listed_only_when_configured() {
        let idp = mock_idp().await;
        let (server, _guard) = server_with_idp(&idp).await;
        let listed: Value = server
            .get("/remux/sso/providers")
            .await
            .json();
        assert_eq!(
            listed,
            json!([{ "Name": "authentik", "DisplayName": "Authentik" }])
        );

        let (plain, _guard) = new_test_server()
            .await
            .unwrap();
        let listed: Value = plain
            .get("/remux/sso/providers")
            .await
            .json();
        assert_eq!(listed, json!([]));
    }
}
