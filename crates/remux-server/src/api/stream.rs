use axum::{
    extract::{Path, State},
    response::IntoResponse,
};
use axum_extra::extract::Query;
use remux_macros::{get, query};
use uuid::Uuid;

use crate::{OptionExt, ResultExt};
use axum_anyhow::ApiResult as Result;

use crate::{AppState, db, stream::StreamDescriptor};

#[query]
pub struct StreamLinkQuery {
    pub token: Option<String>,
}

/// Proxy any stream stored in `db::Media.stream_info` to the caller.
///
/// Handles all URL schemes transparently via [`crate::stream::StreamSource`].
/// Addon-owned streams (`Opendal`) are dispatched to the addon's `serve_stream`.
///
/// Only for this server's own ffmpeg and ffprobe, which send no credentials:
/// the link must carry the token [`StreamDescriptor::server_input`] puts on it.
/// Stream ids are derived from public data (the title's id and the torrent
/// hash), so without the token anyone could stream through this server, past
/// every user policy.
#[get("/stream/{id}")]
pub async fn stream_proxy(
    headers: axum::http::HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<StreamLinkQuery>,
) -> Result<impl IntoResponse> {
    if !q
        .token
        .as_deref()
        .is_some_and(|token| crate::stream::is_stream_link_token(id, token))
    {
        return Err(anyhow::anyhow!("missing or invalid stream link token"))
            .context_not_found("not found");
    }
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .context_not_found("not found")?;

    let descriptor = media
        .stream_info
        .map(|si| si.descriptor)
        .context_not_found("media has no URL")?;

    if let Some(addon_id) = descriptor.addon_id() {
        let addon = state
            .ctx
            .addons
            .get(addon_id)
            .context_not_found("addon not found")?;
        let stream = addon
            .stream
            .as_ref()
            .context_not_found("addon does not support streams")?;
        return stream
            .serve_stream(&descriptor, &headers)
            .await;
    }

    // Live streams go through the transcoder, never this proxy.
    if matches!(descriptor, StreamDescriptor::Rtsp { .. }) {
        return Err(anyhow::anyhow!("rtsp streams are not proxied"))
            .context_bad_request("RTSP streams play through the transcoder");
    }

    if matches!(descriptor, StreamDescriptor::Torrent { .. }) {
        let cfg = db::Settings::get_config_or_default(
            &state
                .ctx
                .db,
        )
        .await;
        if !cfg
            .p2p_enabled
            .unwrap_or(true)
        {
            return Err(anyhow::anyhow!("P2P disabled")).context_bad_request(
                "P2P streams are disabled by the server administrator",
            );
        }
    }

    descriptor
        .into_source()
        .serve(&state, &headers)
        .await
}

#[cfg(test)]
mod tests {
    use crate::{
        integration_test::{authenticated_server, insert_test_source},
        stream::{StreamDescriptor, stream_link_token},
    };

    /// The proxy serves only links this server made for its own ffmpeg: a
    /// stream id alone, which anyone can derive, gets nothing.
    #[tokio::test]
    async fn stream_proxy_requires_the_servers_own_link_token() {
        let (server, guard, _token) = authenticated_server().await;
        let ctx = &guard.0;
        let dir = tempfile::tempdir().unwrap();
        let file = dir
            .path()
            .join("movie.mkv");
        std::fs::write(&file, b"release").unwrap();
        let mut source = insert_test_source(ctx).await;
        source
            .stream_info
            .as_mut()
            .unwrap()
            .descriptor = StreamDescriptor::Local(file);
        source
            .save(&ctx.db)
            .await
            .unwrap();
        let id = source.id;

        server
            .get(&format!("/stream/{id}"))
            .expect_failure()
            .await
            .assert_status_not_found();
        server
            .get(&format!(
                "/stream/{id}?token={}",
                stream_link_token(uuid::Uuid::new_v4())
            ))
            .expect_failure()
            .await
            .assert_status_not_found();

        let served = server
            .get(&format!("/stream/{id}?token={}", stream_link_token(id)))
            .await;
        served.assert_status_ok();
        assert_eq!(
            served
                .as_bytes()
                .as_ref(),
            b"release"
        );
    }

    #[test]
    fn a_stream_link_token_only_fits_its_own_id() {
        let id = uuid::Uuid::new_v4();
        let token = stream_link_token(id);
        assert!(crate::stream::is_stream_link_token(id, &token));
        assert!(!crate::stream::is_stream_link_token(
            uuid::Uuid::new_v4(),
            &token
        ));
        assert!(!crate::stream::is_stream_link_token(id, ""));
        assert!(!crate::stream::is_stream_link_token(id, "zz"));
        assert!(!crate::stream::is_stream_link_token(
            id,
            &token[..token.len() - 1]
        ));
    }
}
