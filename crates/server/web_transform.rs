use std::{
    collections::HashMap,
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use axum::body::Body;
use axum::response::Response;
use bytes::Bytes;
use http::Request;
use http_body_util::BodyExt;
use tower::{Layer, Service};

use crate::web_patches::{CSS, JS};

// ── Cache ────────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct TransformCache(Arc<Mutex<HashMap<String, Bytes>>>);

impl TransformCache {
    pub fn get(&self, path: &str) -> Option<Bytes> {
        self.0.lock().unwrap().get(path).cloned()
    }
    pub fn insert(&self, path: String, bytes: Bytes) {
        self.0.lock().unwrap().insert(path, bytes);
    }
}

// ── Layer ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TransformLayer {
    cache: TransformCache,
}

impl TransformLayer {
    pub fn new() -> Self {
        Self {
            cache: TransformCache::default(),
        }
    }
}

impl<S> Layer<S> for TransformLayer {
    type Service = TransformService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TransformService {
            inner,
            cache: self.cache.clone(),
        }
    }
}

// ── Service ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TransformService<S> {
    inner: S,
    cache: TransformCache,
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
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let path = req.uri().path().to_string();
        let cache = self.cache.clone();
        let fut = self.inner.call(req);

        Box::pin(async move {
            let response = fut.await?;

            // Only transform HTML — JS/CSS/fonts/images pass through untouched.
            let is_html = response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.contains("html"))
                .unwrap_or(false);

            let (parts, body) = response.into_parts();

            if !is_html {
                let bytes = body.collect().await.map(|c| c.to_bytes()).unwrap_or_default();
                return Ok(Response::from_parts(parts, Body::from(bytes)));
            }

            // Cache hit — reuse previously transformed bytes, rebuild response with
            // fresh headers (ETag, Date, etc. come from `parts`).
            if let Some(cached) = cache.get(&path) {
                let mut response = Response::from_parts(parts, Body::from(cached.clone()));
                response.headers_mut().insert(
                    http::header::CONTENT_LENGTH,
                    http::HeaderValue::from(cached.len()),
                );
                return Ok(response);
            }

            // Buffer → inject → cache
            let bytes = body.collect().await.map(|c| c.to_bytes()).unwrap_or_default();
            let mut html = String::from_utf8_lossy(&bytes).into_owned();

            if !CSS.is_empty() {
                let tag = format!("<style data-remux>{CSS}</style></head>");
                html = html.replace("</head>", &tag);
            }

            if !JS.is_empty() {
                let tag = format!("<script data-remux>{JS}</script></body>");
                html = html.replace("</body>", &tag);
            }

            let out = Bytes::from(html.into_bytes());
            cache.insert(path, out.clone());
            let mut response = Response::from_parts(parts, Body::from(out.clone()));
            response.headers_mut().insert(
                http::header::CONTENT_LENGTH,
                http::HeaderValue::from(out.len()),
            );
            Ok(response)
        })
    }
}
