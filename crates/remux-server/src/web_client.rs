use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use axum::{body::Body, response::Response};
use http::{Request, StatusCode, header};
use tower::{Layer, Service, util::BoxCloneSyncService};
use tower_http::services::ServeDir;

#[cfg(feature = "desktop")]
use crate::embedded_static::EmbeddedDir;
use crate::web_transform::TransformLayer;

const JELLYFIN_ALIAS_PREFIX: &str = "/jellyfin";

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

type StaticService = BoxCloneSyncService<Request<Body>, Response<Body>, Infallible>;

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

    let last_segment = path
        .rsplit('/')
        .next()
        .unwrap_or_default();
    if last_segment.contains('.') {
        path.to_string()
    } else {
        "/index.html".to_string()
    }
}

fn rewrite_request_path(mut req: Request<Body>, new_path: &str) -> Request<Body> {
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
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

fn redirect_to_trailing_slash(path: &str, query: Option<&str>) -> Response<Body> {
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

pub fn normalize_web_client(
    value: Option<crate::api::DefaultWebClient>,
) -> crate::api::DefaultWebClient {
    value.unwrap_or_default()
}

#[derive(Clone)]
pub struct WebClientService {
    jellyfin: StaticService,
}

impl WebClientService {
    pub fn from_filesystem(web_path: &str) -> Self {
        let jellyfin = BoxCloneSyncService::new(
            TransformLayer::new().layer(ServeDir::new(web_path)),
        );
        Self { jellyfin }
    }
}

#[cfg(feature = "desktop")]
impl WebClientService {
    pub fn from_embedded(jellyfin_web: &'static include_dir::Dir<'static>) -> Self {
        let jellyfin =
            BoxCloneSyncService::new(TransformLayer::new().layer(EmbeddedDir {
                dir: jellyfin_web,
                spa_fallback: false,
            }));
        Self { jellyfin }
    }
}

impl Service<Request<Body>> for WebClientService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let path = req
            .uri()
            .path()
            .to_string();
        let query = req
            .uri()
            .query()
            .map(str::to_owned);

        let is_service_worker_path = path.eq_ignore_ascii_case("/serviceworker.js");
        let jellyfin_inner = strip_prefixed_path(&path, JELLYFIN_ALIAS_PREFIX);
        let mut jellyfin = self
            .jellyfin
            .clone();

        Box::pin(async move {
            if is_service_worker_path {
                return Ok(unregistering_service_worker_response());
            }

            let jellyfin_path = jellyfin_inner
                .map(|p| normalize_spa_inner_path(&p))
                .unwrap_or_else(|| normalize_spa_inner_path(&path));
            let req = rewrite_request_path(req, &jellyfin_path);
            jellyfin
                .call(req)
                .await
        })
    }
}
