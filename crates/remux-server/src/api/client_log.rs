use axum::{Json, body::Bytes, extract::State, response::IntoResponse};
use chrono::Utc;
use http::StatusCode;
use remux_macros::post;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::{AppState, db::auth};
use axum_anyhow::ApiResult as Result;

const MAX_DOCUMENT_SIZE: usize = 1_000_000;

/// Upload a client log document.
/// POST /clientlog/document
#[post("/clientlog/document")]
pub async fn log_document(
    State(state): State<AppState>,
    session: auth::AuthSession,
    body: Bytes,
) -> Result<impl IntoResponse> {
    if body.len() > MAX_DOCUMENT_SIZE {
        return Ok((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("Payload must be less than {MAX_DOCUMENT_SIZE} bytes"),
        )
            .into_response());
    }

    let client_name = session
        .device
        .app_name
        .replace(['/', '\\', ' '], "_");
    let client_version = session
        .device
        .app_version
        .replace(['/', '\\', ' '], "_");
    let timestamp = Utc::now().format("%Y%m%d%H%M%S");
    let id = uuid::Uuid::new_v4().simple();
    let file_name =
        format!("upload_{client_name}_{client_version}_{timestamp}_{id}.log");

    let log_dir = state
        .ctx
        .config
        .data_dir
        .join("logs");
    tokio::fs::create_dir_all(&log_dir)
        .await
        .map_err(|e| anyhow::anyhow!("failed to create log dir: {e}"))?;

    let file_path = log_dir.join(&file_name);
    let mut file = tokio::fs::File::create(&file_path)
        .await
        .map_err(|e| anyhow::anyhow!("failed to create log file: {e}"))?;
    file.write_all(&body)
        .await
        .map_err(|e| anyhow::anyhow!("failed to write log file: {e}"))?;

    info!(
        file_name = %file_name,
        client = %session.device.app_name,
        version = %session.device.app_version,
        "client log uploaded"
    );

    Ok(Json(json!({ "FileName": file_name })).into_response())
}
