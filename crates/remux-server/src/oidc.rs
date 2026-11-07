//! OpenID Connect sign-in: the authorization-code flow with PKCE against a
//! single operator-configured provider (see the `oidc_*` fields on [`Config`]).
//!
//! The ID token is read straight from the token endpoint over the back
//! channel, so — as OIDC Core §3.1.3.7 allows for the code flow — the
//! connection to the configured issuer vouches for it instead of its JWS
//! signature. Every claim check (`iss`, `aud`, `azp`, `exp`, `nonce`) still runs.

use std::{str::FromStr, time::Duration};

use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http::HeaderMap;
use rand::RngCore;
use remux_sdks::remux::Username;
use remux_utils::Secret;
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;
use url::Url;

use crate::Config;

/// How long a started sign-in may sit at the provider before its `state` is
/// forgotten.
pub const PENDING_LOGIN_TTL: Duration = Duration::from_secs(10 * 60);
/// How long the one-time code handed to the completion page stays valid.
pub const LOGIN_CODE_TTL: Duration = Duration::from_secs(2 * 60);
/// Allowed clock drift between us and the provider when checking `exp`.
const CLOCK_SKEW_SECS: i64 = 60;

/// URL slug identifying a provider: lowercase ASCII letters, digits, `-`, `_`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderName(String);

impl ProviderName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ProviderName {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s
            .trim()
            .to_ascii_lowercase();
        ensure!(!s.is_empty(), "provider name must not be empty");
        ensure!(
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "provider name {s:?} may only contain letters, digits, '-' and '_'"
        );
        Ok(Self(s))
    }
}

/// Where the browser lands after signing in: a same-origin absolute path.
/// The completion page prefixes it with the server's base URL, so it can
/// never point at another host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnPath(String);

impl ReturnPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ReturnPath {
    fn default() -> Self {
        Self("/web/".to_string())
    }
}

impl FromStr for ReturnPath {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        ensure!(s.starts_with('/'), "return path must start with '/'");
        ensure!(
            !s.starts_with("//"),
            "return path must not be protocol-relative"
        );
        ensure!(
            !s.contains('\\')
                && !s
                    .chars()
                    .any(char::is_control),
            "return path contains forbidden characters"
        );
        Ok(Self(s.to_string()))
    }
}

/// State kept between redirecting to the provider and its callback, keyed by
/// the `state` parameter.
#[derive(Debug, Clone)]
pub struct PendingLogin {
    pub provider: ProviderName,
    pub nonce: Secret<String>,
    pub code_verifier: Secret<String>,
    pub redirect_uri: Url,
    pub return_to: ReturnPath,
}

/// The redirect that starts a sign-in, plus what to remember for the callback.
#[derive(Debug)]
pub struct LoginRedirect {
    pub authorization_url: Url,
    pub state: String,
    pub pending: PendingLogin,
}

/// Who the provider says signed in.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub subject: String,
    pub username: Username,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderMetadata {
    issuer: String,
    authorization_endpoint: Url,
    token_endpoint: Url,
    userinfo_endpoint: Option<Url>,
    token_endpoint_auth_methods_supported: Option<Vec<String>>,
}

impl ProviderMetadata {
    /// `client_secret_basic` is the spec default when the provider doesn't
    /// list its methods; only fall back to `client_secret_post` when that's
    /// all it accepts.
    fn uses_basic_auth(&self) -> bool {
        match &self.token_endpoint_auth_methods_supported {
            None => true,
            Some(methods) => {
                methods
                    .iter()
                    .any(|m| m == "client_secret_basic")
                    || !methods
                        .iter()
                        .any(|m| m == "client_secret_post")
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
    access_token: Option<Secret<String>>,
}

#[derive(Debug)]
pub struct OidcProvider {
    pub name: ProviderName,
    pub display_name: String,
    pub auto_create_users: bool,
    pub link_existing_users: bool,
    issuer: Url,
    client_id: String,
    client_secret: Option<Secret<String>>,
    scopes: String,
    username_claim: String,
    groups_claim: String,
    admin_groups: Vec<String>,
    allowed_groups: Vec<String>,
    redirect_url: Option<Url>,
    http: reqwest::Client,
    /// Discovered lazily so an unreachable provider can't block startup;
    /// a failed discovery is retried on the next sign-in.
    metadata: OnceCell<ProviderMetadata>,
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn parse_list(value: &Option<String>) -> Vec<String> {
    non_empty(value)
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|g| !g.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_scopes(value: &str) -> String {
    let mut scopes = vec!["openid"];
    for scope in value.split(|c: char| c == ',' || c.is_whitespace()) {
        if !scope.is_empty() && !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    scopes.join(" ")
}

impl OidcProvider {
    /// `Ok(None)` when OIDC isn't configured; an error when it's configured
    /// wrongly, so a typo fails startup instead of the first sign-in.
    pub fn from_config(config: &Config) -> Result<Option<Self>> {
        let (issuer, client_id) = match (
            non_empty(&config.oidc_issuer_url),
            non_empty(&config.oidc_client_id),
        ) {
            (None, None) => return Ok(None),
            (Some(issuer), Some(client_id)) => (issuer, client_id),
            _ => bail!("OIDC_ISSUER_URL and OIDC_CLIENT_ID must be set together"),
        };
        let issuer = Url::parse(issuer).context("invalid OIDC_ISSUER_URL")?;
        let redirect_url = non_empty(&config.oidc_redirect_url)
            .map(|u| Url::parse(u).context("invalid OIDC_REDIRECT_URL"))
            .transpose()?;
        let client_secret = config
            .oidc_client_secret
            .as_ref()
            .filter(|s| {
                !s.expose()
                    .trim()
                    .is_empty()
            })
            .cloned();

        Ok(Some(Self {
            name: config
                .oidc_provider_name
                .parse()
                .context("invalid OIDC_PROVIDER_NAME")?,
            display_name: config
                .oidc_display_name
                .trim()
                .to_string(),
            auto_create_users: config.oidc_auto_create_users,
            link_existing_users: config.oidc_link_existing_users,
            issuer,
            client_id: client_id.to_string(),
            client_secret,
            scopes: parse_scopes(&config.oidc_scopes),
            username_claim: config
                .oidc_username_claim
                .trim()
                .to_string(),
            groups_claim: config
                .oidc_groups_claim
                .trim()
                .to_string(),
            admin_groups: parse_list(&config.oidc_admin_groups),
            allowed_groups: parse_list(&config.oidc_allowed_groups),
            redirect_url,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()?,
            metadata: OnceCell::new(),
        }))
    }

    async fn metadata(&self) -> Result<&ProviderMetadata> {
        self.metadata
            .get_or_try_init(|| async {
                let url = format!(
                    "{}/.well-known/openid-configuration",
                    self.issuer
                        .as_str()
                        .trim_end_matches('/')
                );
                let metadata: ProviderMetadata = self
                    .http
                    .get(&url)
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .with_context(|| format!("OIDC discovery request to {url} failed"))?
                    .json()
                    .await
                    .with_context(|| {
                        format!("invalid OIDC discovery document at {url}")
                    })?;
                ensure!(
                    metadata
                        .issuer
                        .trim_end_matches('/')
                        == self
                            .issuer
                            .as_str()
                            .trim_end_matches('/'),
                    "provider reports issuer {:?}, expected {:?}",
                    metadata.issuer,
                    self.issuer
                        .as_str()
                );
                Ok::<_, anyhow::Error>(metadata)
            })
            .await
    }

    /// The callback URL registered with the provider: the configured one, or
    /// one derived from the request. Deriving from headers is safe here — the
    /// provider rejects any `redirect_uri` that isn't registered.
    pub fn redirect_uri(&self, headers: &HeaderMap) -> Result<Url> {
        if let Some(url) = &self.redirect_url {
            return Ok(url.clone());
        }
        let first = |name: &str| {
            headers
                .get(name)
                .and_then(|v| {
                    v.to_str()
                        .ok()
                })
                .and_then(|v| {
                    v.split(',')
                        .next()
                })
                .map(str::trim)
                .filter(|v| !v.is_empty())
        };
        let proto = first("x-forwarded-proto").unwrap_or("http");
        ensure!(
            proto == "http" || proto == "https",
            "unsupported X-Forwarded-Proto {proto:?}"
        );
        let host = first("x-forwarded-host")
            .or_else(|| first("host"))
            .context("request has no Host header; set OIDC_REDIRECT_URL")?;
        let url = Url::parse(&format!(
            "{proto}://{host}/sso/oid/redirect/{}",
            self.name
                .as_str()
        ))
        .context("cannot build redirect URL from request headers")?;
        Ok(url)
    }

    pub async fn begin_login(
        &self,
        redirect_uri: Url,
        return_to: ReturnPath,
    ) -> Result<LoginRedirect> {
        let metadata = self
            .metadata()
            .await?;
        let state = random_token();
        let nonce = random_token();
        let code_verifier = random_token();

        let mut authorization_url = metadata
            .authorization_endpoint
            .clone();
        authorization_url
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", redirect_uri.as_str())
            .append_pair("scope", &self.scopes)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &pkce_challenge(&code_verifier))
            .append_pair("code_challenge_method", "S256");

        Ok(LoginRedirect {
            authorization_url,
            state,
            pending: PendingLogin {
                provider: self
                    .name
                    .clone(),
                nonce: nonce.into(),
                code_verifier: code_verifier.into(),
                redirect_uri,
                return_to,
            },
        })
    }

    /// Redeem the authorization `code` and work out who signed in.
    pub async fn complete_login(
        &self,
        code: &str,
        pending: &PendingLogin,
    ) -> Result<Identity> {
        let metadata = self
            .metadata()
            .await?;
        let tokens = self
            .exchange_code(metadata, code, pending)
            .await?;

        let id_token = tokens
            .id_token
            .context(
                "token response has no id_token (is the `openid` scope allowed?)",
            )?;
        let mut claims = decode_jwt_claims(&id_token)?;
        let subject = validate_id_token(
            &claims,
            &metadata.issuer,
            &self.client_id,
            pending
                .nonce
                .expose(),
            chrono::Utc::now().timestamp(),
        )?;

        if let (Some(endpoint), Some(access_token)) =
            (&metadata.userinfo_endpoint, &tokens.access_token)
        {
            // Groups are often only in userinfo. Failing here instead of
            // carrying on without them keeps a flaky provider from silently
            // demoting admins or locking out allowed users.
            let userinfo: Map<String, Value> = self
                .http
                .get(endpoint.clone())
                .bearer_auth(access_token.expose())
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .context("OIDC userinfo request failed")?
                .json()
                .await
                .context("invalid OIDC userinfo response")?;
            ensure!(
                userinfo
                    .get("sub")
                    .and_then(Value::as_str)
                    == Some(subject.as_str()),
                "userinfo subject does not match the ID token"
            );
            claims.extend(userinfo);
        }

        identity_from_claims(subject, &claims, &self.username_claim, &self.groups_claim)
    }

    async fn exchange_code(
        &self,
        metadata: &ProviderMetadata,
        code: &str,
        pending: &PendingLogin,
    ) -> Result<TokenResponse> {
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            (
                "redirect_uri",
                pending
                    .redirect_uri
                    .as_str(),
            ),
            (
                "code_verifier",
                pending
                    .code_verifier
                    .expose()
                    .as_str(),
            ),
        ];
        let mut request = self
            .http
            .post(
                metadata
                    .token_endpoint
                    .clone(),
            )
            .header(http::header::ACCEPT, "application/json");
        match &self.client_secret {
            None => form.push((
                "client_id",
                self.client_id
                    .as_str(),
            )),
            // RFC 6749 §2.3.1: form-encode both halves before Basic encoding.
            Some(secret) if metadata.uses_basic_auth() => {
                request = request.basic_auth(
                    form_encode(&self.client_id),
                    Some(form_encode(secret.expose())),
                );
            }
            Some(secret) => {
                form.push((
                    "client_id",
                    self.client_id
                        .as_str(),
                ));
                form.push((
                    "client_secret",
                    secret
                        .expose()
                        .as_str(),
                ));
            }
        }

        let response = request
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(serde_urlencoded::to_string(&form)?)
            .send()
            .await
            .context("OIDC token request failed")?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_default();
            let reason = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("error_description")
                        .or_else(|| v.get("error"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| {
                    body.chars()
                        .take(200)
                        .collect()
                });
            bail!("OIDC token endpoint returned {status}: {reason}");
        }
        response
            .json()
            .await
            .context("invalid OIDC token response")
    }

    /// Whether this identity may sign in at all.
    pub fn is_allowed(&self, identity: &Identity) -> bool {
        self.allowed_groups
            .is_empty()
            || self.in_any(identity, &self.allowed_groups)
            || self.in_any(identity, &self.admin_groups)
    }

    /// The admin flag to apply, or `None` when admin groups aren't configured
    /// and the local flag is left alone.
    pub fn admin_flag(&self, identity: &Identity) -> Option<bool> {
        (!self
            .admin_groups
            .is_empty())
        .then(|| self.in_any(identity, &self.admin_groups))
    }

    fn in_any(&self, identity: &Identity, groups: &[String]) -> bool {
        identity
            .groups
            .iter()
            .any(|g| groups.contains(g))
    }
}

/// 32 random bytes, URL-safe base64 — used for `state`, `nonce`, PKCE
/// verifiers and one-time login codes.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn form_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn decode_jwt_claims(token: &str) -> Result<Map<String, Value>> {
    let parts: Vec<&str> = token
        .split('.')
        .collect();
    match parts.len() {
        3 => {}
        5 => bail!("encrypted ID tokens are not supported"),
        _ => bail!("malformed ID token"),
    }
    let payload = URL_SAFE_NO_PAD
        .decode(parts[1].trim_end_matches('='))
        .context("ID token payload is not base64url")?;
    serde_json::from_slice(&payload).context("ID token payload is not a JSON object")
}

/// OIDC Core §3.1.3.7 checks; returns the subject.
fn validate_id_token(
    claims: &Map<String, Value>,
    issuer: &str,
    client_id: &str,
    nonce: &str,
    now: i64,
) -> Result<String> {
    let str_claim = |name: &str| {
        claims
            .get(name)
            .and_then(Value::as_str)
    };

    ensure!(
        str_claim("iss") == Some(issuer),
        "ID token issuer {:?} does not match {issuer:?}",
        str_claim("iss")
    );

    let audiences: Vec<&str> = match claims.get("aud") {
        Some(Value::String(aud)) => vec![aud.as_str()],
        Some(Value::Array(auds)) => auds
            .iter()
            .filter_map(Value::as_str)
            .collect(),
        _ => bail!("ID token has no audience"),
    };
    ensure!(
        audiences.contains(&client_id),
        "ID token was not issued for this client"
    );
    match str_claim("azp") {
        Some(azp) => ensure!(azp == client_id, "ID token authorized party mismatch"),
        None => ensure!(
            audiences.len() == 1,
            "ID token has several audiences but no authorized party"
        ),
    }

    let exp = claims
        .get("exp")
        .and_then(Value::as_i64)
        .context("ID token has no expiry")?;
    ensure!(exp + CLOCK_SKEW_SECS >= now, "ID token has expired");

    ensure!(str_claim("nonce") == Some(nonce), "ID token nonce mismatch");

    str_claim("sub")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .context("ID token has no subject")
}

/// Look up a claim by exact name first (namespaced claims such as
/// `https://example.com/groups` contain dots), then as a dotted path.
fn lookup_claim<'a>(claims: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    if let Some(value) = claims.get(path) {
        return Some(value);
    }
    let mut segments = path.split('.');
    let mut value = claims.get(segments.next()?)?;
    for segment in segments {
        value = value.get(segment)?;
    }
    Some(value)
}

fn identity_from_claims(
    subject: String,
    claims: &Map<String, Value>,
    username_claim: &str,
    groups_claim: &str,
) -> Result<Identity> {
    let username = [username_claim, "preferred_username", "email"]
        .into_iter()
        .filter_map(|claim| lookup_claim(claims, claim)?.as_str())
        .find(|v| {
            !v.trim()
                .is_empty()
        })
        .with_context(|| format!("provider sent no {username_claim:?} claim"))?;
    let username = Username::try_new(username).map_err(|e| {
        anyhow!("username {username:?} from the provider is invalid: {e}")
    })?;

    let groups = match lookup_claim(claims, groups_claim) {
        Some(Value::Array(groups)) => groups
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(group)) => vec![group.clone()],
        _ => Vec::new(),
    };

    Ok(Identity {
        subject,
        username,
        groups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claims(value: Value) -> Map<String, Value> {
        value
            .as_object()
            .unwrap()
            .clone()
    }

    fn valid_claims() -> Map<String, Value> {
        claims(json!({
            "iss": "https://idp.example.com",
            "sub": "user-1",
            "aud": "remux",
            "exp": 1_000,
            "nonce": "n0nce",
        }))
    }

    fn validate(claims: &Map<String, Value>) -> Result<String> {
        validate_id_token(claims, "https://idp.example.com", "remux", "n0nce", 900)
    }

    #[test]
    fn pkce_challenge_is_unpadded_base64url_sha256() {
        // Expected value from Python's hashlib + base64.urlsafe_b64encode.
        assert_eq!(
            pkce_challenge("remux-pkce-test-verifier"),
            "nwzLO-xdIuyC9_-iyCfak5exT9q2Z_rvgwoa4vBPWsQ"
        );
    }

    #[test]
    fn provider_name_is_lowercased_and_restricted() {
        assert_eq!(
            "Authentik"
                .parse::<ProviderName>()
                .unwrap()
                .as_str(),
            "authentik"
        );
        assert!(
            "my-idp_2"
                .parse::<ProviderName>()
                .is_ok()
        );
        assert!(
            "a/b"
                .parse::<ProviderName>()
                .is_err()
        );
        assert!(
            "".parse::<ProviderName>()
                .is_err()
        );
    }

    #[test]
    fn return_path_rejects_anything_but_same_origin_paths() {
        assert!(
            "/admin/"
                .parse::<ReturnPath>()
                .is_ok()
        );
        for bad in [
            "https://evil.example",
            "//evil.example",
            "/\\evil.example",
            "web/",
            "/web/\n",
        ] {
            assert!(
                bad.parse::<ReturnPath>()
                    .is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn scopes_always_include_openid_once() {
        assert_eq!(parse_scopes("profile, email"), "openid profile email");
        assert_eq!(parse_scopes("openid groups"), "openid groups");
    }

    #[test]
    fn lists_split_on_commas_and_keep_spaces_inside_names() {
        assert_eq!(
            parse_list(&Some(" Remux Admins, ops ,,".into())),
            vec!["Remux Admins", "ops"]
        );
        assert!(parse_list(&None).is_empty());
    }

    #[test]
    fn valid_id_token_yields_subject() {
        assert_eq!(validate(&valid_claims()).unwrap(), "user-1");
    }

    #[test]
    fn id_token_with_wrong_issuer_audience_or_nonce_is_rejected() {
        for (claim, value) in [
            ("iss", json!("https://other.example.com")),
            ("aud", json!("someone-else")),
            ("nonce", json!("replayed")),
            ("exp", json!(800)),
        ] {
            let mut c = valid_claims();
            c.insert(claim.into(), value);
            assert!(validate(&c).is_err(), "{claim} should be checked");
        }
        let mut c = valid_claims();
        c.remove("nonce");
        assert!(validate(&c).is_err());
    }

    #[test]
    fn expiry_tolerates_clock_skew() {
        let mut c = valid_claims();
        c.insert("exp".into(), json!(900 - CLOCK_SKEW_SECS + 1));
        assert!(validate(&c).is_ok());
    }

    #[test]
    fn multiple_audiences_require_matching_azp() {
        let mut c = valid_claims();
        c.insert("aud".into(), json!(["remux", "other"]));
        assert!(validate(&c).is_err());
        c.insert("azp".into(), json!("remux"));
        assert!(validate(&c).is_ok());
        c.insert("azp".into(), json!("other"));
        assert!(validate(&c).is_err());
    }

    #[test]
    fn jwt_payload_is_decoded_without_verifying_signature() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"abc"}"#);
        let decoded = decode_jwt_claims(&format!("e30.{payload}.sig")).unwrap();
        assert_eq!(decoded["sub"], "abc");
        assert!(decode_jwt_claims("a.b.c.d.e").is_err());
        assert!(decode_jwt_claims("nope").is_err());
    }

    #[test]
    fn identity_reads_nested_groups_and_falls_back_to_email() {
        let c = claims(json!({
            "email": "alice@example.com",
            "realm_access": { "roles": ["admins", "users"] },
        }));
        let id = identity_from_claims(
            "s".into(),
            &c,
            "preferred_username",
            "realm_access.roles",
        )
        .unwrap();
        assert_eq!(
            id.username
                .as_str(),
            "alice@example.com"
        );
        assert_eq!(id.groups, vec!["admins", "users"]);
    }

    #[test]
    fn identity_accepts_namespaced_claims_and_single_group_strings() {
        let c = claims(json!({
            "preferred_username": "bob",
            "https://example.com/groups": "media",
        }));
        let id = identity_from_claims(
            "s".into(),
            &c,
            "preferred_username",
            "https://example.com/groups",
        )
        .unwrap();
        assert_eq!(id.groups, vec!["media"]);
    }

    #[test]
    fn identity_without_username_is_rejected() {
        assert!(
            identity_from_claims(
                "s".into(),
                &Map::new(),
                "preferred_username",
                "groups"
            )
            .is_err()
        );
        let c = claims(json!({ "preferred_username": "bad/name" }));
        assert!(
            identity_from_claims("s".into(), &c, "preferred_username", "groups")
                .is_err()
        );
    }

    fn provider(admin: Option<&str>, allowed: Option<&str>) -> OidcProvider {
        OidcProvider::from_config(&Config {
            oidc_issuer_url: Some("https://idp.example.com".into()),
            oidc_client_id: Some("remux".into()),
            oidc_admin_groups: admin.map(Into::into),
            oidc_allowed_groups: allowed.map(Into::into),
            ..Default::default()
        })
        .unwrap()
        .unwrap()
    }

    fn identity(groups: &[&str]) -> Identity {
        Identity {
            subject: "s".into(),
            username: Username::try_new("u").unwrap(),
            groups: groups
                .iter()
                .map(|g| g.to_string())
                .collect(),
        }
    }

    #[test]
    fn group_rules_gate_access_and_sync_admin() {
        let open = provider(None, None);
        assert!(open.is_allowed(&identity(&[])));
        assert_eq!(open.admin_flag(&identity(&["admins"])), None);

        let gated = provider(Some("admins"), Some("media"));
        assert!(!gated.is_allowed(&identity(&["other"])));
        assert!(gated.is_allowed(&identity(&["media"])));
        // Admins may sign in even when not listed as allowed.
        assert!(gated.is_allowed(&identity(&["admins"])));
        assert_eq!(gated.admin_flag(&identity(&["media"])), Some(false));
        assert_eq!(gated.admin_flag(&identity(&["admins"])), Some(true));
    }

    #[test]
    fn config_requires_issuer_and_client_together() {
        assert!(
            OidcProvider::from_config(&Config::default())
                .unwrap()
                .is_none()
        );
        let half = Config {
            oidc_issuer_url: Some("https://idp.example.com".into()),
            ..Default::default()
        };
        assert!(OidcProvider::from_config(&half).is_err());
    }

    #[test]
    fn redirect_uri_prefers_config_then_forwarded_headers() {
        let p = provider(None, None);
        let mut headers = HeaderMap::new();
        headers.insert(
            "host",
            "internal:3000"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            p.redirect_uri(&headers)
                .unwrap()
                .as_str(),
            "http://internal:3000/sso/oid/redirect/oidc"
        );
        headers.insert(
            "x-forwarded-proto",
            "https"
                .parse()
                .unwrap(),
        );
        headers.insert(
            "x-forwarded-host",
            "media.example.com"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            p.redirect_uri(&headers)
                .unwrap()
                .as_str(),
            "https://media.example.com/sso/oid/redirect/oidc"
        );

        let configured = OidcProvider {
            redirect_url: Some(Url::parse("https://fixed.example.com/cb").unwrap()),
            ..p
        };
        assert_eq!(
            configured
                .redirect_uri(&headers)
                .unwrap()
                .as_str(),
            "https://fixed.example.com/cb"
        );
    }
}
