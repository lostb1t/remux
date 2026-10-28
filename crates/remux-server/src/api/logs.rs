use axum::{
    Json,
    extract::{Query, State},
};
use remux_macros::{get, query};
use serde::Serialize;
use std::{collections::VecDeque, path::PathBuf, time::SystemTime};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::{AppState, db::auth};
use axum_anyhow::ApiResult as Result;

const MAX_LOG_FILES: usize = 14;
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 1000;

#[query]
pub struct GetLogsQuery {
    pub limit: Option<usize>,
    pub level: Option<String>,
    pub search: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct LogsResult {
    pub entries: Vec<LogEntry>,
}

/// Read a bounded tail of the server's rolling logs. Admin-only: logs may contain diagnostics.
#[get("/remux/logs")]
pub async fn get_logs(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Query(query): Query<GetLogsQuery>,
) -> Result<Json<LogsResult>> {
    let log_dir = state
        .ctx
        .config
        .data_dir
        .join("logs");
    let mut dir = match tokio::fs::read_dir(&log_dir).await {
        Ok(dir) => dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Json(LogsResult {
                entries: Vec::new(),
            }));
        }
        Err(error) => {
            return Err(anyhow::anyhow!("failed to read log directory: {error}").into());
        }
    };

    let mut files: Vec<(SystemTime, PathBuf)> = Vec::new();
    while let Some(entry) = dir
        .next_entry()
        .await?
    {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("remux") || !name.ends_with(".log") {
            continue;
        }
        if let Ok(metadata) = entry
            .metadata()
            .await
        {
            if metadata.is_file() {
                files.push((
                    metadata
                        .modified()
                        .unwrap_or(SystemTime::UNIX_EPOCH),
                    path,
                ));
            }
        }
    }
    files.sort_by(|a, b| {
        b.0.cmp(&a.0)
    });
    files.truncate(MAX_LOG_FILES);

    let wanted_level = query
        .level
        .as_deref()
        .unwrap_or("all")
        .to_ascii_lowercase();
    let needle = query
        .search
        .as_deref()
        .unwrap_or("")
        .to_lowercase();
    let limit = query
        .limit
        .unwrap_or(500)
        .clamp(1, MAX_ENTRIES);
    let mut entries = VecDeque::with_capacity(limit);
    for (_, path) in files
        .into_iter()
        .rev()
    {
        let content = match read_log_tail(path).await {
            Ok(content) => content,
            Err(_) => continue,
        };
        for line in content.lines() {
            let (timestamp, level, message) = parse_log_line(line);
            if wanted_level != "all" && level.to_ascii_lowercase() != wanted_level {
                continue;
            }
            if !needle.is_empty()
                && !line
                    .to_lowercase()
                    .contains(&needle)
            {
                continue;
            }
            entries.push_back(LogEntry {
                timestamp,
                level,
                message,
            });
            if entries.len() > limit {
                entries.pop_front();
            }
        }
    }
    Ok(Json(LogsResult {
        entries: entries.into(),
    }))
}

async fn read_log_tail(path: PathBuf) -> std::io::Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let length = file
        .metadata()
        .await?
        .len();
    let offset = length.saturating_sub(MAX_LOG_BYTES);
    if offset > 0 {
        file.seek(std::io::SeekFrom::Start(offset))
            .await?;
    }
    let mut bytes = Vec::with_capacity(length.min(MAX_LOG_BYTES) as usize);
    file.read_to_end(&mut bytes)
        .await?;
    let mut content = String::from_utf8_lossy(&bytes).into_owned();
    if offset > 0 {
        if let Some(newline) = content.find('\n') {
            content.drain(..=newline);
        } else {
            content.clear();
        }
    }
    Ok(content)
}

fn parse_log_line(line: &str) -> (String, String, String) {
    // The compact tracing formatter starts with HH:MM:SS LEVEL target: message.
    let mut parts = line.splitn(3, ' ');
    let first = parts
        .next()
        .unwrap_or("");
    let second = parts
        .next()
        .unwrap_or("");
    let third = parts
        .next()
        .unwrap_or("");
    let (timestamp, level, rest) =
        if ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"].contains(&second) {
            (first.to_string(), second.to_string(), third)
        } else {
            (String::new(), "INFO".to_string(), line)
        };
    let message = rest
        .split_once(": ")
        .map(|(_, message)| message)
        .unwrap_or(rest)
        .to_string();
    (timestamp, level, message)
}

#[cfg(test)]
mod tests {
    use super::{MAX_LOG_BYTES, parse_log_line, read_log_tail};

    #[test]
    fn parses_compact_tracing_lines() {
        assert_eq!(
            parse_log_line(
                "12:34:56 ERROR remux_server::api: failed request: timed out"
            ),
            (
                "12:34:56".into(),
                "ERROR".into(),
                "failed request: timed out".into()
            )
        );
    }

    #[test]
    fn keeps_unrecognized_lines_visible() {
        assert_eq!(
            parse_log_line("plain output"),
            ("".into(), "INFO".into(), "plain output".into())
        );
    }

    #[tokio::test]
    async fn reads_bounded_tail_without_returning_a_partial_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("remux.log");
        let mut content = b"old entry\n".to_vec();
        content.extend_from_slice(
            "x\n"
                .repeat(MAX_LOG_BYTES as usize / 2)
                .as_bytes(),
        );
        content.extend_from_slice(b"\nlatest entry\n");
        tokio::fs::write(&path, content)
            .await
            .unwrap();

        let tail = read_log_tail(path)
            .await
            .unwrap();
        assert!(tail.starts_with("x\n"));
        assert!(tail.ends_with("latest entry\n"));
        assert!(!tail.contains("old entry"));
    }
}
