use axum::extract::{Path, State};
use axum::response::IntoResponse;
use remux_macros::get;
use uuid::Uuid;

use axum_anyhow::{ApiResult as Result, OptionExt};

use crate::AppState;
use crate::db;
use crate::stream::StreamDescriptor;

/// Proxy any stream stored in `db::Media.url` to the caller.
///
/// Handles all URL schemes transparently via [`crate::stream::StreamSource`].
/// Addon-owned streams (`Opendal`) are dispatched to the addon's `serve_stream`.
/// Auth is not required — stream UUIDs are stable but not guessable.
#[get("/stream/{id}")]
pub async fn stream_proxy(
    headers: axum::http::HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("stream", "not found")?;

    let descriptor = media.url.context_not_found("stream", "media has no URL")?;

    if let Some(addon_id) = descriptor.addon_id() {
        let addon = state
            .ctx
            .addons
            .get(addon_id)
            .await
            .context_not_found("stream", "addon not found")?;
        return addon.kind.serve_stream(&descriptor, &headers).await;
    }

    descriptor.into_source().serve(&state, &headers).await
}
