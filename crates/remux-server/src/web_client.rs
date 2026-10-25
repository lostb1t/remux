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

/// `GET /`: sends the visitor to the configured default web client. Falls
/// back to Jellyfin Web when Remux Web is chosen but not installed, so a
/// server without it never lands on a 404.
pub async fn root_redirect(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    uri: axum::http::Uri,
) -> axum::response::Redirect {
    // Read per request, so a change in the dashboard applies at once.
    let chosen = normalize_web_client(
        crate::db::Settings::get_config_or_default(
            &state
                .ctx
                .db,
        )
        .await
        .default_web_client,
    );
    let installed = state
        .ctx
        .web_paths
        .as_ref()
        .is_some_and(|p| {
            std::path::Path::new(&p.remux_web_path)
                .join("index.html")
                .is_file()
        });
    let client = match chosen {
        crate::api::DefaultWebClient::Remux if !installed => {
            crate::api::DefaultWebClient::Jellyfin
        }
        other => other,
    };
    let query = uri
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    // Temporary: the target changes with the setting, and a cached permanent
    // redirect would keep sending browsers to the old client.
    axum::response::Redirect::temporary(&format!("{}{query}", client.path()))
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

impl WebClientService {
    /// Like [`Self::from_filesystem`] without the custom script injection:
    /// for clients that bring their own UI (Remux Web).
    pub fn from_filesystem_plain(web_path: &str) -> Self {
        let inner = BoxCloneSyncService::new(
            TransformLayer::new(None).layer(ServeDir::new(web_path)),
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

#[cfg(test)]
mod tests {
    use axum_test::TestServer;

    use crate::{Config, FilesystemPaths, admin_from_filesystem, api, db, init_app};

    use super::WebClientService;

    /// A server whose Remux Web client lives in `remux_web`, with `chosen`
    /// as the configured default web client.
    async fn server(
        remux_web: &std::path::Path,
        chosen: api::DefaultWebClient,
    ) -> (TestServer, crate::AppContext) {
        let paths = FilesystemPaths {
            remux_web_path: remux_web
                .to_str()
                .unwrap()
                .to_string(),
            ..Default::default()
        };
        let config = Config {
            database_url: Some("sqlite::memory:".into()),
            torrent_http_port: None,
            torrent_peer_port: None,
            disable_dht: true,
            disable_startup_tasks: true,
            ..Default::default()
        };
        let admin = admin_from_filesystem(&paths.dashboard_path);
        let web_path = paths
            .web_path
            .clone();
        let (router, ctx) = init_app(config, Some(paths), admin, move |pool| {
            WebClientService::from_filesystem(&web_path, pool)
        })
        .await
        .unwrap();
        let mut cfg = db::Settings::get_config_or_default(&ctx.db).await;
        cfg.default_web_client = Some(chosen);
        db::Settings::set_config(&ctx.db, &cfg)
            .await
            .unwrap();
        let server = TestServer::builder()
            .mock_transport()
            .build(router)
            .unwrap();
        (server, ctx)
    }

    fn location(resp: &axum_test::TestResponse) -> String {
        resp.header("location")
            .to_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn root_opens_jellyfin_web_unless_remux_web_is_chosen_and_installed() {
        let dir = tempfile::tempdir().unwrap();

        let (jellyfin, _ctx) =
            server(dir.path(), api::DefaultWebClient::Jellyfin).await;
        let resp = jellyfin
            .get("/")
            .await;
        assert_eq!(resp.status_code(), http::StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(location(&resp), "/web/");

        // Chosen but not installed: never a 404, fall back to Jellyfin Web.
        let (missing, _ctx) = server(dir.path(), api::DefaultWebClient::Remux).await;
        assert_eq!(
            location(
                &missing
                    .get("/")
                    .await
            ),
            "/web/"
        );

        std::fs::write(
            dir.path()
                .join("index.html"),
            "remux shell",
        )
        .unwrap();
        let (remux, _ctx) = server(dir.path(), api::DefaultWebClient::Remux).await;
        assert_eq!(
            location(
                &remux
                    .get("/")
                    .await
            ),
            "/remux/"
        );
        assert_eq!(
            location(
                &remux
                    .get("/?server=abc")
                    .await
            ),
            "/remux/?server=abc",
            "the query survives, like the Jellyfin Web redirect"
        );
    }

    #[tokio::test]
    async fn remux_web_is_served_at_remux_with_spa_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path()
                .join("index.html"),
            "remux shell",
        )
        .unwrap();
        std::fs::create_dir(
            dir.path()
                .join("assets"),
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join("assets/app.js"),
            "console.log(1)",
        )
        .unwrap();
        let (server, _ctx) = server(dir.path(), api::DefaultWebClient::Jellyfin).await;

        let index = server
            .get("/remux/")
            .await;
        assert_eq!(index.status_code(), http::StatusCode::OK);
        assert_eq!(index.text(), "remux shell");
        assert_eq!(
            server
                .get("/remux/assets/app.js")
                .await
                .text(),
            "console.log(1)"
        );
        // A client route without a file extension gets the shell.
        assert_eq!(
            server
                .get("/remux/library/42")
                .await
                .text(),
            "remux shell"
        );
    }

    /// The real server wraps the router in `rewrite_request_uri`. Without
    /// `/remux` among its mount prefixes, `/remux/` became `/remux`, which the
    /// web client service redirects back to `/remux/`: a redirect loop.
    #[tokio::test]
    async fn remux_web_root_is_served_through_the_uri_rewrite_layer() {
        use tower::{Layer, ServiceExt, util::MapRequestLayer};

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path()
                .join("index.html"),
            "remux shell",
        )
        .unwrap();
        let paths = FilesystemPaths {
            remux_web_path: dir
                .path()
                .to_str()
                .unwrap()
                .to_string(),
            ..Default::default()
        };
        let config = Config {
            database_url: Some("sqlite::memory:".into()),
            torrent_http_port: None,
            torrent_peer_port: None,
            disable_dht: true,
            disable_startup_tasks: true,
            ..Default::default()
        };
        let admin = admin_from_filesystem(&paths.dashboard_path);
        let web_path = paths
            .web_path
            .clone();
        let (router, _ctx) = init_app(config, Some(paths), admin, move |pool| {
            WebClientService::from_filesystem(&web_path, pool)
        })
        .await
        .unwrap();
        let app = MapRequestLayer::new(crate::rewrite_request_uri).layer(router);

        for (path, status) in [
            ("/remux/", http::StatusCode::OK),
            // Without the slash it redirects once, to the slash form.
            ("/remux", http::StatusCode::PERMANENT_REDIRECT),
            ("/remux/library/42", http::StatusCode::OK),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    http::Request::get(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), status, "{path}");
            if status == http::StatusCode::PERMANENT_REDIRECT {
                assert_eq!(resp.headers()["location"], "/remux/");
            }
        }
    }

    #[test]
    fn default_web_client_round_trips_and_falls_back_to_jellyfin() {
        for client in api::DefaultWebClient::ALL {
            assert_eq!(
                api::DefaultWebClient::from_str_lossy(client.as_str()),
                client
            );
        }
        assert_eq!(
            api::DefaultWebClient::from_str_lossy(" Remux "),
            api::DefaultWebClient::Remux
        );
        assert_eq!(
            api::DefaultWebClient::from_str_lossy("anfiteatro"),
            api::DefaultWebClient::Jellyfin
        );
    }
}
