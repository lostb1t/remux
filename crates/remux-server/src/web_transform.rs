use std::{
    collections::HashMap,
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use axum::{body::Body, response::Response};
use bytes::Bytes;
use http::Request;
use http_body_util::BodyExt;
use tower::{Layer, Service};

use crate::web_patches::{CSS, JS};

const BRANDING_CONFIG_KEY: &str = "branding_configuration";

#[derive(Clone, Default)]
pub struct TransformCache(Arc<Mutex<HashMap<String, Bytes>>>);

impl TransformCache {
    pub fn get(&self, path: &str) -> Option<Bytes> {
        self.0
            .lock()
            .unwrap()
            .get(path)
            .cloned()
    }
    pub fn insert(&self, path: String, bytes: Bytes) {
        self.0
            .lock()
            .unwrap()
            .insert(path, bytes);
    }
}

#[derive(Clone)]
pub struct TransformLayer {
    cache: TransformCache,
    pool: Option<sqlx::SqlitePool>,
}

impl TransformLayer {
    pub fn new(pool: Option<sqlx::SqlitePool>) -> Self {
        Self {
            cache: TransformCache::default(),
            pool,
        }
    }
}

impl<S> Layer<S> for TransformLayer {
    type Service = TransformService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TransformService {
            inner,
            cache: self
                .cache
                .clone(),
            pool: self
                .pool
                .clone(),
        }
    }
}

#[derive(Clone)]
pub struct TransformService<S> {
    inner: S,
    cache: TransformCache,
    pool: Option<sqlx::SqlitePool>,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for TransformService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: http_body::Body<Data = Bytes> + Send + 'static,
    ResBody::Error: std::error::Error + Send + Sync,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner
            .poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        let path = req
            .uri()
            .path()
            .to_string();
        // The inner file service validates `If-Modified-Since` / `If-None-Match`
        // against the file on disk, but the injected patches change without the
        // file changing (server update, custom JS). Let pages always be served
        // fresh so browsers never keep a stale copy.
        if serves_page(&path) {
            let headers = req.headers_mut();
            headers.remove(http::header::IF_MODIFIED_SINCE);
            headers.remove(http::header::IF_NONE_MATCH);
        }
        let cache = self
            .cache
            .clone();
        let pool = self
            .pool
            .clone();
        let fut = self
            .inner
            .call(req);

        Box::pin(async move {
            let response = fut.await?;

            // Only transform HTML — JS/CSS/fonts/images pass through untouched.
            let is_html = response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| {
                    v.to_str()
                        .ok()
                })
                .map(|ct| ct.contains("html"))
                .unwrap_or(false);

            let (parts, body) = response.into_parts();

            if !is_html {
                let bytes = body
                    .collect()
                    .await
                    .map(|c| c.to_bytes())
                    .unwrap_or_default();
                return Ok(Response::from_parts(parts, Body::from(bytes)));
            }

            // Buffer → inject
            let bytes = body
                .collect()
                .await
                .map(|c| c.to_bytes())
                .unwrap_or_default();
            let mut html = String::from_utf8_lossy(&bytes).into_owned();

            if !CSS.is_empty() {
                let tag = format!("<style data-remux>{CSS}</style></head>");
                html = html.replace("</head>", &tag);
            }

            let user_js = match pool.as_ref() {
                Some(p) => custom_js_from_db(p).await,
                None => None,
            };

            if !JS.is_empty() || user_js.is_some() {
                let extra = user_js
                    .as_deref()
                    .unwrap_or("");
                let tag = format!("<script data-remux>{JS}{extra}</script></body>");
                html = html.replace("</body>", &tag);
            }

            let out = Bytes::from(html.into_bytes());
            let mut response = Response::from_parts(parts, Body::from(out.clone()));
            let headers = response.headers_mut();
            headers.insert(
                http::header::CONTENT_LENGTH,
                http::HeaderValue::from(out.len()),
            );
            // The file's validators don't describe the patched document.
            headers.remove(http::header::LAST_MODIFIED);
            headers.remove(http::header::ETAG);
            headers.insert(
                http::header::CACHE_CONTROL,
                http::HeaderValue::from_static("no-cache"),
            );
            Ok(response)
        })
    }
}

/// Paths that resolve to an HTML document: directories, `.html` files and
/// extension-less SPA routes. Assets keep their conditional requests.
fn serves_page(path: &str) -> bool {
    let last = path
        .rsplit('/')
        .next()
        .unwrap_or("");
    last.is_empty()
        || !last.contains('.')
        || last
            .to_ascii_lowercase()
            .ends_with(".html")
}

async fn custom_js_from_db(pool: &sqlx::SqlitePool) -> Option<String> {
    let json = crate::db::Settings::get(pool, BRANDING_CONFIG_KEY)
        .await
        .ok()??;
    let opts: remux_sdks::remux::BrandingOptions = serde_json::from_str(&json).ok()?;
    opts.remux?
        .custom_js
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    use tower_http::services::ServeDir;

    fn dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path()
                .join("index.html"),
            "<html><head></head><body></body></html>",
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join("app.js"),
            "console.log(1)",
        )
        .unwrap();
        dir
    }

    async fn get(dir: &tempfile::TempDir, path: &str) -> Response {
        TransformLayer::new(None)
            .layer(ServeDir::new(dir.path()))
            .oneshot(
                Request::get(path)
                    .header(
                        http::header::IF_MODIFIED_SINCE,
                        "Fri, 01 Jan 2100 00:00:00 GMT",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn patched_pages_are_never_answered_from_the_browser_cache() {
        let dir = dir();
        for path in ["/", "/index.html"] {
            let res = get(&dir, path).await;
            assert_eq!(res.status(), http::StatusCode::OK, "{path}");
            assert!(
                res.headers()
                    .get(http::header::LAST_MODIFIED)
                    .is_none()
            );
            assert_eq!(res.headers()[http::header::CACHE_CONTROL], "no-cache");
            let body = res
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes();
            assert!(String::from_utf8_lossy(&body).contains("data-remux"));
        }
    }

    #[tokio::test]
    async fn assets_keep_conditional_requests() {
        let res = get(&dir(), "/app.js").await;
        assert_eq!(res.status(), http::StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn page_paths() {
        assert!(serves_page("/"));
        assert!(serves_page("/web/"));
        assert!(serves_page("/index.html"));
        assert!(serves_page("/home"));
        assert!(!serves_page("/main.bundle.js"));
        assert!(!serves_page("/assets/style.css"));
    }
}
