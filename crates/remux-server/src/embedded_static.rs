use std::convert::Infallible;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use bytes::Bytes;
use include_dir::Dir;
use tower::Service;

fn mime_for_path(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[derive(Clone)]
pub struct EmbeddedDir {
    pub dir: &'static Dir<'static>,
    /// If true, unknown paths fall back to index.html (SPA behaviour).
    pub spa_fallback: bool,
}

impl<B> Service<Request<B>> for EmbeddedDir {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        std::future::ready(Ok(self.serve(req.uri().path())))
    }
}

impl EmbeddedDir {
    fn serve(&self, uri_path: &str) -> Response<Body> {
        let path = uri_path.trim_start_matches('/');
        let path = if path.is_empty() { "index.html" } else { path };

        if let Some(file) = self.dir.get_file(path) {
            return self.file_response(path, file.contents());
        }

        if self.spa_fallback {
            if let Some(index) = self.dir.get_file("index.html") {
                return self.file_response("index.html", index.contents());
            }
        }

        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap()
    }

    fn file_response(&self, path: &str, contents: &'static [u8]) -> Response<Body> {
        Response::builder()
            .header(header::CONTENT_TYPE, mime_for_path(path))
            .body(Body::from(Bytes::from_static(contents)))
            .unwrap()
    }
}
