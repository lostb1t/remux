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

use crate::web_patches::PATCHES;

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

            // Only transform JS and HTML
            let is_text = response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.contains("javascript") || ct.contains("html"))
                .unwrap_or(false);

            let (parts, body) = response.into_parts();

            if !is_text || PATCHES.is_empty() {
                let bytes = body
                    .collect()
                    .await
                    .map(|c| c.to_bytes())
                    .unwrap_or_default();
                return Ok(Response::from_parts(parts, Body::from(bytes)));
            }

            // Cache hit
            if let Some(cached) = cache.get(&path) {
                return Ok(Response::from_parts(parts, Body::from(cached)));
            }

            // Buffer → transform → cache
            let bytes = body
                .collect()
                .await
                .map(|c| c.to_bytes())
                .unwrap_or_default();
            let mut text = String::from_utf8_lossy(&bytes).into_owned();

            for patch in PATCHES {
                if text.contains(patch.search) {
                    text = text.replace(patch.search, patch.replace);
                }
            }

            let out = Bytes::from(text.into_bytes());
            cache.insert(path, out.clone());
            Ok(Response::from_parts(parts, Body::from(out)))
        })
    }
}
