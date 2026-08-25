use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use axum::{body::Body, extract::OriginalUri, response::Response};
use http::{Request, StatusCode, header};
use tower::{Layer, Service, util::BoxCloneSyncService};
use tower_http::services::ServeDir;

#[cfg(feature = "desktop")]
use crate::embedded_static::EmbeddedDir;
use crate::web_transform::TransformLayer;

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

fn spa_path(path: &str) -> &str {
    // Paths without a file extension are SPA navigation routes — serve the shell.
    let last_segment = path
        .rsplit('/')
        .next()
        .unwrap_or_default();
    if last_segment.contains('.') {
        path
    } else {
        "/index.html"
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

/// Unregisters any root-scoped service worker left over from older installs that
/// served `serviceworker.js` at `/` instead of `/web/`.
pub async fn root_serviceworker() -> Response<Body> {
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
        .unwrap()
}

pub fn normalize_web_client(
    value: Option<crate::api::DefaultWebClient>,
) -> crate::api::DefaultWebClient {
    value.unwrap_or_default()
}

#[derive(Clone)]
pub struct WebClientService {
    inner: StaticService,
}

impl WebClientService {
    pub fn from_filesystem(web_path: &str, pool: sqlx::SqlitePool) -> Self {
        let inner = BoxCloneSyncService::new(
            TransformLayer::new(Some(pool)).layer(ServeDir::new(web_path)),
        );
        Self { inner }
    }
}

#[cfg(feature = "desktop")]
impl WebClientService {
    pub fn from_embedded(
        jellyfin_web: &'static include_dir::Dir<'static>,
        pool: sqlx::SqlitePool,
    ) -> Self {
        let inner = BoxCloneSyncService::new(TransformLayer::new(Some(pool)).layer(
            EmbeddedDir {
                dir: jellyfin_web,
                spa_fallback: false,
            },
        ));
        Self { inner }
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
        let original_path = req
            .extensions()
            .get::<OriginalUri>()
            .map(|u| {
                u.path()
                    .to_string()
            });
        let mut inner = self
            .inner
            .clone();

        Box::pin(async move {
            // Redirect /web → /web/ and /jellyfin → /jellyfin/ so relative asset
            // URLs in index.html resolve correctly against the mounted prefix.
            if path == "/" {
                if let Some(orig) = &original_path {
                    if !orig.ends_with('/') {
                        let q = req
                            .uri()
                            .query()
                            .map(|q| format!("?{q}"))
                            .unwrap_or_default();
                        let redirect = format!("{orig}/{q}");
                        return Ok(Response::builder()
                            .status(StatusCode::PERMANENT_REDIRECT)
                            .header(header::LOCATION, redirect)
                            .body(Body::empty())
                            .unwrap());
                    }
                }
            }

            if path.eq_ignore_ascii_case("/serviceworker.js") {
                return Ok(Response::builder()
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
                    .unwrap_or_else(|_| {
                        Response::new(Body::from(UNREGISTER_SW_SCRIPT))
                    }));
            }

            let req = rewrite_request_path(req, spa_path(&path));
            inner
                .call(req)
                .await
        })
    }
}
