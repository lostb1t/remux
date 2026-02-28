use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::StreamExt;
use futures_util::stream;
use http::StatusCode;
use remux_macros::{get, post};
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;

use crate::AppState;
use crate::db::auth;
use axum_anyhow::{ApiResult as Result, IntoApiError, OptionExt, ResultExt};

#[derive(Deserialize)]
pub struct LogStreamQuery {
    token: Option<String>,
}

#[derive(Deserialize)]
pub struct SetLogLevelRequest {
    pub level: String,
}

/// GET /logs/stream?token=...
/// SSE endpoint — streams log lines as JSON. Auth via `token` query param.
#[get("/logs/stream")]
pub async fn log_stream(
    State(state): State<AppState>,
    Query(q): Query<LogStreamQuery>,
) -> Result<impl IntoResponse> {
    let token = q
        .token
        .context_unauthorized("unauthorized", "missing token")?;

    auth::Device::get_by_access_token(&state.ctx.db, &token)
        .await?
        .context_unauthorized("unauthorized", "invalid token")?;

    // Subscribe FIRST so no live events are missed during file read.
    let rx = crate::log_capture::subscribe()
        .context_unauthorized("unavailable", "log capture not initialized")?;

    let history = if let Some(path) = crate::log_capture::log_file_path() {
        read_tail(path, 1000).await.unwrap_or_default()
    } else {
        vec![]
    };

    let history_stream = stream::iter(history)
        .map(|json| Ok::<Event, std::convert::Infallible>(Event::default().data(json)));

    let live_stream = BroadcastStream::new(rx).filter_map(|item| async move {
        let line = item.ok()?;
        let data = serde_json::to_string(&line).ok()?;
        Some(Ok::<Event, std::convert::Infallible>(Event::default().data(data)))
    });

    Ok(Sse::new(history_stream.chain(live_stream))
        .keep_alive(KeepAlive::default())
        .into_response())
}

/// POST /system/log/level
/// Change the remux_server log level at runtime. Admin only.
#[post("/system/log/level")]
pub async fn set_log_level(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(body): Json<SetLogLevelRequest>,
) -> Result<impl IntoResponse> {
    if !session.user.is_admin {
        return Err(anyhow::anyhow!("Admin access required")
            .context_forbidden("forbidden", "admin required"));
    }

    let level = body.level.to_lowercase();
    match level.as_str() {
        "trace" | "debug" | "info" | "warn" | "error" => {}
        _ => {
            return Err(anyhow::anyhow!("Invalid log level: {level}")
                .context_bad_request("invalid", "level must be trace/debug/info/warn/error"));
        }
    }

    crate::log_capture::set_log_level(&level)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn read_tail(path: &str, n: usize) -> anyhow::Result<Vec<String>> {
    use tokio::io::AsyncBufReadExt;
    let file = tokio::fs::File::open(path).await?;
    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();
    let mut buf = std::collections::VecDeque::with_capacity(n);
    while let Some(line) = lines.next_line().await? {
        if !line.is_empty() {
            if buf.len() >= n {
                buf.pop_front();
            }
            buf.push_back(line);
        }
    }
    Ok(buf.into_iter().collect())
}
