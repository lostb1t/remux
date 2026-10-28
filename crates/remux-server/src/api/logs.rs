use axum::{
    Json,
    extract::{Query, State},
};
use remux_macros::{get, query};
use serde::Serialize;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::SystemTime,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::{AppState, db::auth};
use axum_anyhow::ApiResult as Result;

const MAX_LOG_FILES: usize = 14;
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_EXPLICIT_LIMIT: usize = 1000;

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
    let entries = load_log_entries(&log_dir, &query).await?;
    Ok(Json(LogsResult { entries }))
}

async fn load_log_entries(
    log_dir: &Path,
    query: &GetLogsQuery,
) -> std::io::Result<Vec<LogEntry>> {
    let mut dir = match tokio::fs::read_dir(log_dir).await {
        Ok(dir) => dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
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
        .map(|limit| limit.clamp(1, MAX_EXPLICIT_LIMIT));
    let mut entries = VecDeque::new();
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
            if let Some(limit) = limit {
                if entries.len() > limit {
                    entries.pop_front();
                }
            }
        }
    }
    let mut entries: Vec<_> = entries
        .into_iter()
        .collect();
    entries.reverse();
    Ok(entries)
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
    // split_whitespace handles the formatter's level-column padding.
    let mut parts = line.split_whitespace();
    let first = parts
        .next()
        .unwrap_or("");
    let second = parts
        .next()
        .unwrap_or("");
    let (timestamp, level, rest) =
        if ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"].contains(&second) {
            let rest_start = line
                .find(second)
                .unwrap_or(line.len())
                + second.len();
            (
                first.to_string(),
                second.to_string(),
                line[rest_start..].trim_start(),
            )
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
    use super::{MAX_LOG_BYTES, load_log_entries, parse_log_line, read_log_tail};

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
    fn parses_compact_lines_with_formatter_padding() {
        assert_eq!(
            parse_log_line(
                "12:34:56  ERROR remux_server::api: failed request: timed out"
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
    async fn returns_all_matching_entries_newest_first_when_limit_is_omitted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("remux.log");
        let content = (0..1105)
            .map(|index| format!("12:34:{:02} INFO remux: entry-{index}\n", index % 60))
            .collect::<String>();
        tokio::fs::write(path, content)
            .await
            .unwrap();

        let entries = load_log_entries(
            dir.path(),
            &super::GetLogsQuery {
                limit: None,
                level: None,
                search: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(entries.len(), 1105);
        assert_eq!(
            entries
                .first()
                .unwrap()
                .message,
            "entry-1104"
        );
        assert_eq!(
            entries
                .last()
                .unwrap()
                .message,
            "entry-0"
        );
    }

    #[tokio::test]
    async fn applies_explicit_limit_after_ordering_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("remux.log");
        let content = (0..5)
            .map(|index| format!("12:34:{:02} INFO remux: entry-{index}\n", index))
            .collect::<String>();
        tokio::fs::write(path, content)
            .await
            .unwrap();

        let entries = load_log_entries(
            dir.path(),
            &super::GetLogsQuery {
                limit: Some(2),
                level: None,
                search: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry
                    .message
                    .as_str())
                .collect::<Vec<_>>(),
            ["entry-4", "entry-3"]
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
