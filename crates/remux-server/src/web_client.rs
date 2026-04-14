use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use axum::{body::Body, response::Response};
use bytes::Bytes;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use tower::{Layer, Service};
use tower::util::BoxCloneSyncService;

#[cfg(feature = "desktop")]
use std::sync::Arc;
#[cfg(feature = "desktop")]
use tokio::sync::RwLock;

#[cfg(feature = "desktop")]
use crate::embedded_static::EmbeddedDir;
#[cfg(not(feature = "desktop"))]
use crate::AppContext;
use crate::web_transform::TransformLayer;

#[cfg(not(feature = "desktop"))]
use tower_http::services::ServeDir;

pub const WEB_CLIENT_JELLYFIN: &str = "jellyfin";
pub const WEB_CLIENT_ANFITEATRO: &str = "anfiteatro";

const ANFITEATRO_PREFIX: &str = "/anfiteatro";
const JELLYFIN_ALIAS_PREFIX: &str = "/jellyfin";
const MOUNT_PREFIX: &str = "/anfi";

const UNREGISTER_SW_SCRIPT: &str = r#"self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (event) => {
    event.waitUntil((async () => {
        try {
            const keys = await caches.keys();
            await Promise.all(keys.map((k) => caches.delete(k)));
        } catch (_) {}

        await self.registration.unregister();

        const clients = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
        for (const client of clients) {
            client.navigate(client.url);
        }
    })());
});
self.addEventListener('fetch', () => {});
"#;

// Best-effort runtime cleanup injected into Anfiteatro HTML to remove stale
// service workers/caches that may still control localhost scope.
const ANFITEATRO_SW_CLEANUP_JS: &str = r#"(function(){
    try {
        if (!('serviceWorker' in navigator)) return;
        navigator.serviceWorker.getRegistrations()
            .then(function(regs){ return Promise.all(regs.map(function(r){ return r.unregister(); })); })
            .catch(function(){});
    } catch (_) {}
    try {
        if (!('caches' in window)) return;
        caches.keys()
            .then(function(keys){ return Promise.all(keys.map(function(k){ return caches.delete(k); })); })
            .catch(function(){});
    } catch (_) {}
})();"#;

type StaticService =
    BoxCloneSyncService<Request<Body>, Response<Body>, Infallible>;

fn strip_mount_prefix(path: &str) -> String {
    // Allow remux to run behind a `/anfi` reverse-proxy prefix.
    let lower = path.to_ascii_lowercase();
    if lower == MOUNT_PREFIX || lower == format!("{MOUNT_PREFIX}/") {
        return "/".to_string();
    }

    let prefix_with_slash = format!("{MOUNT_PREFIX}/");
    if lower.starts_with(&prefix_with_slash) {
        return path[MOUNT_PREFIX.len()..].to_string();
    }

    path.to_string()
}

fn strip_prefixed_path(path: &str, prefix: &str) -> Option<String> {
    // Convert `/prefix/*` routes back to a root path for static services.
    let lower = path.to_ascii_lowercase();
    let prefix_lower = prefix.to_ascii_lowercase();

    if lower == prefix_lower || lower == format!("{prefix_lower}/") {
        return Some("/".to_string());
    }

    let prefix_with_slash = format!("{prefix_lower}/");
    if lower.starts_with(&prefix_with_slash) {
        return Some(path[prefix.len()..].to_string());
    }

    None
}

fn normalize_spa_inner_path(path: &str) -> String {
    // For SPA navigation paths, serve the static shell.
    if path == "/" {
        return "/index.html".to_string();
    }

    let last_segment = path.rsplit('/').next().unwrap_or_default();
    if last_segment.contains('.') {
        path.to_string()
    } else {
        "/index.html".to_string()
    }
}

fn rewrite_request_path(mut req: Request<Body>, new_path: &str) -> Request<Body> {
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    if let Ok(uri) = format!("{new_path}{query}").parse() {
        *req.uri_mut() = uri;
    }
    req
}

fn unregistering_service_worker_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )
        .header(
            header::CACHE_CONTROL,
            "no-store, no-cache, must-revalidate, max-age=0",
        )
        .body(Body::from(UNREGISTER_SW_SCRIPT))
        .unwrap_or_else(|_| Response::new(Body::from(UNREGISTER_SW_SCRIPT)))
}

fn redirect_to_trailing_slash(
    path: &str,
    query: Option<&str>,
) -> Response<Body> {
    let location = match query {
        Some(q) if !q.is_empty() => format!("{path}/?{q}"),
        _ => format!("{path}/"),
    };

    Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header(header::LOCATION, location)
        .body(Body::empty())
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

async fn inject_anfiteatro_runtime_guards(
    response: Response<Body>,
) -> Response<Body> {
    // Only mutate HTML responses; static assets pass through untouched.
    let is_html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("html"))
        .unwrap_or(false);
    if !is_html {
        return response;
    }

    let (parts, body) = response.into_parts();
    let bytes = body
        .collect()
        .await
        .map(|c| c.to_bytes())
        .unwrap_or_default();
    let mut html = String::from_utf8_lossy(&bytes).into_owned();

    if !html.contains("data-remux-sw-cleanup") {
        let tag = format!(
            "<script data-remux-sw-cleanup>{ANFITEATRO_SW_CLEANUP_JS}</script></body>"
        );
        html = html.replace("</body>", &tag);
    }

    let out = Bytes::from(html.into_bytes());
    let mut response = Response::from_parts(parts, Body::from(out.clone()));
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        http::HeaderValue::from(out.len()),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        http::HeaderValue::from_static(
            "no-store, no-cache, must-revalidate, max-age=0",
        ),
    );
    response
}

pub fn normalize_web_client(
    value: Option<crate::api::DefaultWebClient>,
) -> crate::api::DefaultWebClient {
    value.unwrap_or_default()
}

#[derive(Clone)]
pub struct DynamicWebClientService {
    jellyfin: StaticService,
    anfiteatro: Option<StaticService>,
}

#[cfg(not(feature = "desktop"))]
impl DynamicWebClientService {
    pub fn from_filesystem(ctx: &AppContext) -> Self {
        let jellyfin = TransformLayer::new()
            .layer(ServeDir::new(ctx.config.web_path.clone()));
        let jellyfin = BoxCloneSyncService::new(jellyfin);

        let anfiteatro = if std::path::Path::new(&ctx.config.anfiteatro_web_path)
            .join("index.html")
            .exists()
        {
            Some(
                BoxCloneSyncService::new(
                    TransformLayer::new().layer(ServeDir::new(
                        ctx.config.anfiteatro_web_path.clone(),
                    )),
                ),
            )
        } else {
            tracing::warn!(
                path = %ctx.config.anfiteatro_web_path,
                "anfiteatro web client not found; using jellyfin web client"
            );
            None
        };

        Self {
            jellyfin,
            anfiteatro,
        }
    }
}

#[cfg(feature = "desktop")]
impl DynamicWebClientService {
    pub fn from_embedded(
        _default_web_client: Arc<RwLock<String>>,
        jellyfin_web: &'static include_dir::Dir<'static>,
        anfiteatro_web: Option<&'static include_dir::Dir<'static>>,
    ) -> Self {
        let jellyfin = TransformLayer::new()
            .layer(EmbeddedDir {
                dir: jellyfin_web,
                spa_fallback: false,
            });
        let jellyfin = BoxCloneSyncService::new(jellyfin);

        let anfiteatro = anfiteatro_web.map(|dir| {
            BoxCloneSyncService::new(TransformLayer::new().layer(EmbeddedDir {
                    dir,
                    spa_fallback: false,
                }))
        });

        if anfiteatro.is_none() {
            tracing::warn!(
                "embedded anfiteatro web client not found; using jellyfin web client"
            );
        }

        Self {
            jellyfin,
            anfiteatro,
        }
    }
}

impl Service<Request<Body>> for DynamicWebClientService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let raw_path = req.uri().path().to_string();
        let query = req.uri().query().map(str::to_owned);
        let path = strip_mount_prefix(&raw_path);
        if path.eq_ignore_ascii_case(ANFITEATRO_PREFIX) && !raw_path.ends_with('/') {
            let response = redirect_to_trailing_slash(&raw_path, query.as_deref());
            return Box::pin(async move { Ok(response) });
        }

        let anfiteatro_inner = strip_prefixed_path(&path, ANFITEATRO_PREFIX);
        let jellyfin_inner = strip_prefixed_path(&path, JELLYFIN_ALIAS_PREFIX);
        let is_anfiteatro_root_asset = {
            let lower = path.to_ascii_lowercase();
            lower == "/_expo" || lower.starts_with("/_expo/")
        };
        let is_service_worker_path = {
            let root_sw = path.eq_ignore_ascii_case("/serviceworker.js");
            let anfiteatro_sw = anfiteatro_inner
                .as_deref()
                .is_some_and(|p| p.eq_ignore_ascii_case("/serviceworker.js"));
            root_sw || anfiteatro_sw
        };
        let mut jellyfin = self.jellyfin.clone();
        let mut anfiteatro = self.anfiteatro.clone();

        Box::pin(async move {
            if is_service_worker_path {
                return Ok(unregistering_service_worker_response());
            }

            if is_anfiteatro_root_asset {
                if let Some(mut service) = anfiteatro.take() {
                    let anfiteatro_path = normalize_spa_inner_path(&path);
                    let req = rewrite_request_path(req, &anfiteatro_path);
                    let response = service.call(req).await?;
                    return Ok(inject_anfiteatro_runtime_guards(response).await);
                }
            }

            if let Some(inner_path) = anfiteatro_inner {
                if let Some(mut service) = anfiteatro.take() {
                    let anfiteatro_path = normalize_spa_inner_path(&inner_path);
                    let req = rewrite_request_path(req, &anfiteatro_path);
                    let response = service.call(req).await?;
                    return Ok(inject_anfiteatro_runtime_guards(response).await);
                }

                tracing::warn!(
                    requested_path = %path,
                    "anfiteatro route requested but web client not available; falling back to jellyfin"
                );
            }

            let jellyfin_path = jellyfin_inner
                .map(|p| normalize_spa_inner_path(&p))
                .unwrap_or_else(|| normalize_spa_inner_path(&path));
            let req = rewrite_request_path(req, &jellyfin_path);
            jellyfin.call(req).await
        })
    }
}
