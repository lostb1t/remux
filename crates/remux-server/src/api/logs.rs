use axum::{
    Json,
    extract::{Query, State},
};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use regex::Regex;
use remux_macros::get;
use remux_sdks::remux::{GetLogs, LogEntry, LogLevel, LogsResult};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::LazyLock,
    time::SystemTime,
};

use crate::{AppState, db::auth};
use axum_anyhow::ApiResult as Result;

const MAX_LOG_BYTES_PER_FILE: u64 = 8 * 1024 * 1024;
const DEFAULT_PAGE_SIZE: usize = 200;
const MAX_PAGE_SIZE: usize = 1000;

static SECRET_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(["']?\b(?:authorization|proxy-authorization|x-api-key|api[-_]?key|access[-_]?token|refresh[-_]?token|token|password|passwd|secret|cookie)\b["']?[ \t]*[:=][ \t]*)"#,
    )
    .unwrap()
});
static BARE_BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]+").unwrap());

/// Read the bounded tail of rolling logs. This endpoint is admin-only; common
/// credential-shaped values are redacted, but operators should still treat log
/// messages as sensitive and disable file logging when it is not desired.
#[get("/remux/logs")]
pub async fn get_logs(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Query(query): Query<GetLogs>,
) -> Result<Json<LogsResult>> {
    let config = &state
        .ctx
        .config;
    let log_dir = config
        .log_dir
        .clone()
        .unwrap_or_else(|| {
            config
                .data_dir
                .join("logs")
        });
    let max_files = config.log_file_max_files;
    let entries = tokio::task::spawn_blocking(move || {
        load_log_entries(&log_dir, &query, max_files)
    })
    .await??;
    Ok(Json(entries))
}

fn load_log_entries(
    log_dir: &Path,
    query: &GetLogs,
    max_files: usize,
) -> std::io::Result<LogsResult> {
    let mut files = match fs::read_dir(log_dir) {
        Ok(dir) => dir
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let is_log = name == "remux.log"
                    || (name.starts_with("remux.") && name.ends_with(".log"));
                if !is_log
                    || !entry
                        .file_type()
                        .ok()?
                        .is_file()
                {
                    return None;
                }
                let metadata = entry
                    .metadata()
                    .ok()?;
                Some((
                    metadata
                        .modified()
                        .unwrap_or(SystemTime::UNIX_EPOCH),
                    entry.path(),
                ))
            })
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LogsResult {
                entries: Vec::new(),
                has_more: false,
            });
        }
        Err(error) => return Err(error),
    };
    files.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| {
                b.1.cmp(&a.1)
            })
    });
    if max_files > 0 {
        files.truncate(max_files);
    }

    let needle = query
        .search
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let limit = query
        .limit
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let mut remaining_offset = query
        .offset
        .unwrap_or_default();
    let mut result = Vec::with_capacity(limit);
    let mut has_more = false;

    // Pages stay small even when no limit is supplied. Offset pagination lets
    // an admin request older entries without rendering the full log tail at once.
    for (_, path) in files {
        has_more = match append_newest_log_entries(
            &path,
            &needle,
            query.level,
            &mut remaining_offset,
            limit,
            &mut result,
        ) {
            Ok(has_more) => has_more,
            Err(_) => continue,
        };
        if has_more {
            break;
        }
    }
    Ok(LogsResult {
        entries: result,
        has_more,
    })
}

fn append_newest_log_entries(
    path: &Path,
    needle: &str,
    wanted_level: LogLevel,
    remaining_offset: &mut usize,
    limit: usize,
    result: &mut Vec<LogEntry>,
) -> std::io::Result<bool> {
    let mut lines = ReverseLineReader::new(path)?;
    let date = log_date(path);
    let mut continuations: Vec<String> = Vec::new();

    while let Some(line) = lines.next_line()? {
        if let Some((timestamp, level, mut message)) = parse_log_line(&line, date) {
            for continuation in continuations
                .iter()
                .rev()
            {
                message.push('\n');
                message.push_str(continuation);
            }
            continuations.clear();
            let entry = LogEntry {
                timestamp,
                level,
                message: redact_sensitive_values(&message),
            };
            if push_matching_entry(
                entry,
                needle,
                wanted_level,
                remaining_offset,
                limit,
                result,
            ) {
                return Ok(true);
            }
        } else {
            continuations.push(redact_sensitive_values(&line));
        }
    }

    // A bounded tail can start in the middle of a multiline event. If no
    // earlier header is available, expose those orphan lines as a plain entry.
    if !continuations.is_empty() {
        let message = continuations
            .iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let entry = LogEntry {
            timestamp: date
                .map(|date| date.to_string())
                .unwrap_or_default(),
            level: "INFO".to_string(),
            message,
        };
        if push_matching_entry(
            entry,
            needle,
            wanted_level,
            remaining_offset,
            limit,
            result,
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn push_matching_entry(
    entry: LogEntry,
    needle: &str,
    wanted_level: LogLevel,
    remaining_offset: &mut usize,
    limit: usize,
    result: &mut Vec<LogEntry>,
) -> bool {
    if !entry_matches(&entry, needle, wanted_level) {
        return false;
    }
    if *remaining_offset > 0 {
        *remaining_offset -= 1;
        return false;
    }
    if result.len() >= limit {
        return true;
    }
    result.push(entry);
    false
}

fn entry_matches(entry: &LogEntry, needle: &str, wanted_level: LogLevel) -> bool {
    level_matches(&entry.level, wanted_level)
        && (needle.is_empty()
            || format!("{} {} {}", entry.timestamp, entry.level, entry.message)
                .to_lowercase()
                .contains(needle))
}

struct ReverseLineReader {
    file: File,
    next_end: u64,
    min_offset: u64,
    buffer: Vec<u8>,
    skip_trailing_empty: bool,
    #[cfg(test)]
    bytes_read: u64,
}

impl ReverseLineReader {
    const CHUNK_SIZE: u64 = 8 * 1024;

    fn new(path: &Path) -> std::io::Result<Self> {
        let mut file = File::open(path)?;
        let length = file
            .metadata()?
            .len();
        let min_offset = length.saturating_sub(MAX_LOG_BYTES_PER_FILE);
        let skip_trailing_empty = if length > 0 {
            file.seek(SeekFrom::Start(length - 1))?;
            let mut last_byte = [0_u8; 1];
            file.read_exact(&mut last_byte)?;
            last_byte[0] == b'\n'
        } else {
            false
        };
        Ok(Self {
            file,
            next_end: length,
            min_offset,
            buffer: Vec::new(),
            skip_trailing_empty,
            #[cfg(test)]
            bytes_read: 0,
        })
    }

    fn next_line(&mut self) -> std::io::Result<Option<String>> {
        loop {
            if let Some(newline) = self
                .buffer
                .iter()
                .rposition(|byte| *byte == b'\n')
            {
                let line = self
                    .buffer
                    .split_off(newline + 1);
                self.buffer
                    .truncate(newline);
                if self.skip_trailing_empty && line.is_empty() {
                    self.skip_trailing_empty = false;
                    continue;
                }
                self.skip_trailing_empty = false;
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }

            if self.next_end == self.min_offset {
                if self
                    .buffer
                    .is_empty()
                    || self.min_offset > 0
                {
                    return Ok(None);
                }
                self.skip_trailing_empty = false;
                return Ok(Some(
                    String::from_utf8_lossy(&std::mem::take(&mut self.buffer))
                        .into_owned(),
                ));
            }

            let start = self
                .next_end
                .saturating_sub(Self::CHUNK_SIZE)
                .max(self.min_offset);
            let mut chunk = vec![0_u8; (self.next_end - start) as usize];
            self.file
                .seek(SeekFrom::Start(start))?;
            self.file
                .read_exact(&mut chunk)?;
            #[cfg(test)]
            {
                self.bytes_read += chunk.len() as u64;
            }
            chunk.extend(std::mem::take(&mut self.buffer));
            self.buffer = chunk;
            self.next_end = start;
        }
    }
}

#[cfg(test)]
fn read_log_tail(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let length = file
        .metadata()?
        .len();
    let offset = length.saturating_sub(MAX_LOG_BYTES_PER_FILE);
    if offset > 0 {
        file.seek(SeekFrom::Start(offset))?;
    }
    let mut bytes = Vec::with_capacity(length.min(MAX_LOG_BYTES_PER_FILE) as usize);
    file.read_to_end(&mut bytes)?;
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

fn log_date(path: &Path) -> Option<NaiveDate> {
    let name = path
        .file_name()?
        .to_str()?;
    let date = name
        .strip_prefix("remux.")?
        .strip_suffix(".log")?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

fn legacy_timestamp_for_file_date<Tz: TimeZone>(
    utc_file_date: NaiveDate,
    local_time: NaiveTime,
    timezone: &Tz,
) -> Option<NaiveDateTime> {
    [
        utc_file_date.pred_opt(),
        Some(utc_file_date),
        utc_file_date.succ_opt(),
    ]
    .into_iter()
    .flatten()
    .find_map(|local_date| {
        let local_datetime = timezone
            .from_local_datetime(&local_date.and_time(local_time))
            .earliest()?;
        (local_datetime
            .with_timezone(&Utc)
            .date_naive()
            == utc_file_date)
            .then_some(local_datetime.naive_local())
    })
}

#[cfg(test)]
fn parse_log_file(content: &str, date: Option<NaiveDate>) -> Vec<LogEntry> {
    let mut entries: Vec<LogEntry> = Vec::new();
    for line in content.lines() {
        if let Some((timestamp, level, message)) = parse_log_line(line, date) {
            entries.push(LogEntry {
                timestamp,
                level,
                message: redact_sensitive_values(&message),
            });
        } else if let Some(previous) = entries.last_mut() {
            previous
                .message
                .push('\n');
            previous
                .message
                .push_str(&redact_sensitive_values(line));
        } else {
            entries.push(LogEntry {
                timestamp: date
                    .map(|d| d.to_string())
                    .unwrap_or_default(),
                level: "INFO".to_string(),
                message: redact_sensitive_values(line),
            });
        }
    }
    entries
}

fn parse_log_line(
    line: &str,
    date: Option<NaiveDate>,
) -> Option<(String, String, String)> {
    let mut parts = line.split_whitespace();
    let time_or_datetime = parts.next()?;
    let level = parts.next()?;
    if !matches!(level, "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR") {
        return None;
    }

    let timestamp = if DateTime::parse_from_rfc3339(time_or_datetime).is_ok() {
        time_or_datetime.to_string()
    } else {
        let time = NaiveTime::parse_from_str(time_or_datetime, "%H:%M:%S%.f").ok()?;
        match date {
            Some(date) => legacy_timestamp_for_file_date(date, time, &Local)
                .map(|timestamp| timestamp.to_string())
                .unwrap_or_else(|| format!("{date} {time}")),
            None => time.to_string(),
        }
    };

    let level_end = line.find(level)? + level.len();
    let target_and_message = line[level_end..].trim_start();
    let message = strip_target_and_span_prefix(target_and_message).to_string();
    Some((timestamp, level.to_string(), message))
}

fn strip_target_and_span_prefix(target_and_message: &str) -> &str {
    let mut search_from = 0;
    while let Some(relative_separator) = target_and_message[search_from..].find(": ") {
        let separator = search_from + relative_separator;
        let prefix = &target_and_message[..separator];
        let has_source_line = prefix
            .rsplit_once(':')
            .is_some_and(|(_, line)| {
                !line.is_empty()
                    && line
                        .chars()
                        .all(|c| c.is_ascii_digit())
            });
        if has_source_line {
            return &target_and_message[separator + 2..];
        }
        search_from = separator + 2;
    }
    target_and_message
        .split_once(": ")
        .map(|(_, message)| message)
        .unwrap_or(target_and_message)
}

fn level_matches(actual: &str, wanted: LogLevel) -> bool {
    if wanted == LogLevel::All {
        return true;
    }
    let rank = |level: &str| match level
        .to_ascii_lowercase()
        .as_str()
    {
        "trace" => Some(0),
        "debug" => Some(1),
        "info" => Some(2),
        "warn" | "warning" => Some(3),
        "error" => Some(4),
        _ => None,
    };
    let wanted = match wanted {
        LogLevel::All => return true,
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    };
    match (rank(actual), rank(wanted)) {
        (Some(actual), Some(wanted)) => actual >= wanted,
        _ => false,
    }
}

fn redact_sensitive_values(message: &str) -> String {
    let mut redacted = String::with_capacity(message.len());
    let mut cursor = 0;
    for captures in SECRET_ASSIGNMENT.captures_iter(message) {
        let prefix = captures
            .get(1)
            .unwrap();
        if prefix.start() < cursor {
            continue;
        }
        redacted.push_str(&message[cursor..prefix.end()]);
        let rest = &message[prefix.end()..];
        let Some(first) = rest
            .as_bytes()
            .first()
            .copied()
        else {
            cursor = prefix.end();
            continue;
        };

        let (value_len, replacement) = if first == 34 || first == 39 {
            let mut index = 1;
            let mut closing = None;
            while index < rest.len() {
                let byte = rest.as_bytes()[index];
                if byte == 92 {
                    index = (index + 2).min(rest.len());
                    continue;
                }
                if byte == first {
                    closing = Some(index + 1);
                    break;
                }
                if byte == b'\n' {
                    break;
                }
                index += 1;
            }
            if let Some(end) = closing {
                let quote = char::from(first);
                (end, format!("{quote}[REDACTED]{quote}"))
            } else {
                (
                    rest.find('\n')
                        .unwrap_or(rest.len()),
                    format!("{}[REDACTED]", char::from(first)),
                )
            }
        } else {
            let mut token_start = 0;
            if rest
                .get(..6)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("Bearer"))
            {
                let mut after_scheme = 6;
                while matches!(
                    rest.as_bytes()
                        .get(after_scheme),
                    Some(b' ' | b'\t')
                ) {
                    after_scheme += 1;
                }
                if after_scheme > 6 {
                    token_start = after_scheme;
                }
            }
            let mut end = token_start;
            while let Some(byte) = rest
                .as_bytes()
                .get(end)
            {
                if matches!(
                    *byte,
                    b' ' | b'\t'
                        | b'\r'
                        | b'\n'
                        | b','
                        | b'&'
                        | b';'
                        | b'"'
                        | b'\''
                        | b'}'
                ) {
                    break;
                }
                end += 1;
            }
            if end > token_start {
                (end, "[REDACTED]".to_string())
            } else {
                (0, String::new())
            }
        };
        redacted.push_str(&replacement);
        cursor = prefix.end() + value_len;
    }
    redacted.push_str(&message[cursor..]);
    BARE_BEARER
        .replace_all(&redacted, "Bearer [REDACTED]")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        LogLevel, MAX_LOG_BYTES_PER_FILE, level_matches, load_log_entries,
        parse_log_file, parse_log_line, read_log_tail,
    };
    use axum::http::{HeaderValue, StatusCode, header::AUTHORIZATION};
    use chrono::NaiveDate;
    use remux_sdks::remux::{GetLogs, LogsResult};
    use serde_json::json;
    use std::{fs, path::Path};

    #[test]
    fn parses_timestamp_date_padding_and_target() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        assert_eq!(
            parse_log_line(
                "12:34:56  ERROR remux_server::api: failed request: timed out",
                Some(date)
            ),
            Some((
                "2026-10-28 12:34:56".into(),
                "ERROR".into(),
                "failed request: timed out".into()
            ))
        );
    }

    #[test]
    fn includes_multiline_continuations_and_redacts_credentials() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        let entries = parse_log_file(
            "12:34:56 ERROR remux::api: request failed Authorization: Bearer topsecret\n  at server.rs:12\nurl=https://host/?api_key=abc123",
            Some(date),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp, "2026-10-28 12:34:56");
        assert_eq!(
            entries[0].message,
            "request failed Authorization: [REDACTED]\n  at server.rs:12\nurl=https://host/?api_key=[REDACTED]"
        );
        assert!(
            !entries[0]
                .message
                .contains("topsecret")
        );
        assert!(
            !entries[0]
                .message
                .contains("abc123")
        );
    }

    #[test]
    fn removes_span_and_target_prefixes_without_truncating_message_colons() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        assert_eq!(
            parse_log_line(
                "12:34:56 INFO request:refresh_streams: remux_server::addons:3472: streams synced: 3",
                Some(date)
            )
            .unwrap()
            .2,
            "streams synced: 3"
        );
    }

    #[test]
    fn resolves_legacy_local_times_against_utc_rolling_file_dates() {
        use chrono::{FixedOffset, NaiveTime};

        let utc_file_date = NaiveDate::from_ymd_opt(2026, 10, 27).unwrap();
        let local_midnight = NaiveTime::from_hms_opt(0, 30, 0).unwrap();
        let utc_plus_one = FixedOffset::east_opt(3600).unwrap();
        assert_eq!(
            super::legacy_timestamp_for_file_date(
                utc_file_date,
                local_midnight,
                &utc_plus_one
            ),
            Some(
                NaiveDate::from_ymd_opt(2026, 10, 28)
                    .unwrap()
                    .and_time(local_midnight)
            )
        );

        let utc_file_date = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        let local_late_evening = NaiveTime::from_hms_opt(23, 30, 0).unwrap();
        let utc_minus_five = FixedOffset::west_opt(5 * 3600).unwrap();
        assert_eq!(
            super::legacy_timestamp_for_file_date(
                utc_file_date,
                local_late_evening,
                &utc_minus_five
            ),
            Some(
                NaiveDate::from_ymd_opt(2026, 10, 27)
                    .unwrap()
                    .and_time(local_late_evening)
            )
        );
    }

    #[test]
    fn redacts_json_quoted_sensitive_values() {
        let message =
            r#"payload={"ApiKey":"x","Authorization":"Bearer y","password":"z"}"#;
        let redacted = super::redact_sensitive_values(message);
        assert_eq!(
            redacted,
            r#"payload={"ApiKey":"[REDACTED]","Authorization":"[REDACTED]","password":"[REDACTED]"}"#
        );
    }

    #[test]
    fn redacts_complete_quoted_secret_values() {
        let message = r#"payload={"password":"my pass","apiKey":"x,y","token":"quote \" suffix"}"#;
        assert_eq!(
            super::redact_sensitive_values(message),
            r#"payload={"password":"[REDACTED]","apiKey":"[REDACTED]","token":"[REDACTED]"}"#
        );
    }

    #[test]
    fn levels_include_more_severe_entries() {
        assert!(level_matches("ERROR", LogLevel::Warn));
        assert!(level_matches("WARN", LogLevel::Warn));
        assert!(!level_matches("INFO", LogLevel::Warn));
        assert!(level_matches("TRACE", LogLevel::All));
    }

    #[tokio::test]
    async fn defaults_to_a_bounded_newest_first_page_when_limit_is_omitted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("remux.2026-10-28.log");
        let content = (0..1105)
            .map(|index| format!("12:34:{:02} INFO remux: entry-{index}\n", index % 60))
            .collect::<String>();
        tokio::fs::write(&path, content)
            .await
            .unwrap();
        let entries = load_log_entries(
            &path
                .parent()
                .unwrap(),
            &GetLogs::default(),
            5,
        )
        .unwrap();
        assert_eq!(
            entries
                .entries
                .len(),
            200
        );
        assert!(entries.has_more);
        assert_eq!(
            entries
                .entries
                .first()
                .unwrap()
                .message,
            "entry-1104"
        );
        assert_eq!(
            entries
                .entries
                .last()
                .unwrap()
                .message,
            "entry-905"
        );
        let older = load_log_entries(
            &path
                .parent()
                .unwrap(),
            &GetLogs {
                limit: Some(200),
                offset: Some(200),
                ..Default::default()
            },
            5,
        )
        .unwrap();
        assert_eq!(
            older
                .entries
                .len(),
            200
        );
        assert_eq!(
            older
                .entries
                .first()
                .unwrap()
                .message,
            "entry-904"
        );
        assert_eq!(
            older
                .entries
                .last()
                .unwrap()
                .message,
            "entry-705"
        );
        assert!(older.has_more);
    }

    #[test]
    fn explicit_limit_is_applied_after_filtering_and_stops_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("remux.2026-10-28.log"),
            "12:34:51 INFO remux: match one\n12:34:52 WARN remux: match two\n12:34:53 ERROR remux: match three\n",
        ).unwrap();
        let entries = load_log_entries(
            dir.path(),
            &GetLogs {
                limit: Some(1),
                offset: None,
                level: LogLevel::Warn,
                search: Some("match".into()),
            },
            5,
        )
        .unwrap();
        assert_eq!(
            entries
                .entries
                .len(),
            1
        );
        assert_eq!(entries.entries[0].message, "match three");
    }

    #[test]
    fn reverse_reader_starts_at_latest_line_without_scanning_the_whole_file() {
        use super::ReverseLineReader;

        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("remux.2026-10-28.log");
        let content = format!(
            "{}12:34:59 INFO remux: newest\n",
            "12:34:00 INFO remux: older\n".repeat(2_000)
        );
        fs::write(&path, &content).unwrap();
        let mut reader = ReverseLineReader::new(&path).unwrap();

        assert_eq!(
            reader
                .next_line()
                .unwrap()
                .as_deref(),
            Some("12:34:59 INFO remux: newest")
        );
        assert!(reader.bytes_read < content.len() as u64);
    }

    #[test]
    fn endpoint_loader_keeps_multiline_entries_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("remux.2026-10-28.log");
        fs::write(
            &path,
            "12:34:51 INFO remux: older\n12:34:52 ERROR remux: newest\ntrace frame one\ntrace frame two\n",
        )
        .unwrap();
        let entries = load_log_entries(dir.path(), &GetLogs::default(), 5).unwrap();

        assert_eq!(
            entries
                .entries
                .len(),
            2
        );
        assert_eq!(
            entries.entries[0].message,
            "newest\ntrace frame one\ntrace frame two"
        );
        assert_eq!(entries.entries[1].message, "older");
    }

    #[tokio::test]
    async fn reads_bounded_tail_without_partial_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("remux.2026-10-28.log");
        let mut content = b"old entry\n".to_vec();
        content.extend_from_slice(
            "x\n"
                .repeat(MAX_LOG_BYTES_PER_FILE as usize / 2)
                .as_bytes(),
        );
        content.extend_from_slice(b"\nlatest entry\n");
        tokio::fs::write(&path, content)
            .await
            .unwrap();
        let tail = read_log_tail(&path).unwrap();
        assert!(tail.starts_with("x\n"));
        assert!(tail.ends_with("latest entry\n"));
        assert!(!tail.contains("old entry"));
    }

    async fn logs_server() -> (
        axum_test::TestServer,
        crate::integration_test::TestGuard,
        tempfile::TempDir,
        String,
    ) {
        use crate::Config;
        use crate::integration_test::{AUTH_HEADER, new_test_server_with_config};
        use http::header::HeaderValue;
        use remux_sdks::remux::AuthenticationInfo;

        let temp = tempfile::tempdir().unwrap();
        let log_dir = temp
            .path()
            .join("logs");
        fs::create_dir_all(&log_dir).unwrap();
        let config = Config {
            database_url: Some("sqlite::memory:".into()),
            log_dir: Some(log_dir),
            log_file_enabled: false,
            torrent_http_port: None,
            torrent_peer_port: None,
            disable_dht: true,
            disable_startup_tasks: true,
            ..Default::default()
        };
        let (server, guard) = new_test_server_with_config(config)
            .await
            .unwrap();
        let response = server
            .post("/users/authenticatebyname")
            .add_header(AUTHORIZATION, HeaderValue::from_static(AUTH_HEADER))
            .json(&json!({"Username":"test","Pw":"test"}))
            .await;
        let admin: AuthenticationInfo = response.json();
        (
            server,
            guard,
            temp,
            admin
                .access_token
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn endpoint_requires_admin_and_applies_search_level_and_limit() {
        let (server, _guard, temp, admin_token) = logs_server().await;
        let log_dir = temp
            .path()
            .join("logs");
        fs::write(
            log_dir.join("remux.2026-10-28.log"),
            "12:34:51 INFO remux: plain\n12:34:52 WARN remux: target=one Authorization: Bearer secret\n12:34:53 ERROR remux: target=two\n",
        ).unwrap();

        server
            .get("/remux/logs")
            .expect_failure()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);

        let admin_header =
            crate::integration_test::auth_header_with_token(&admin_token);
        let created = server
            .post("/users/new")
            .add_header(AUTHORIZATION, HeaderValue::from_str(&admin_header).unwrap())
            .json(&json!({"Name":"viewer","Password":"viewer"}))
            .await;
        created.assert_status_ok();
        let auth_header =
            HeaderValue::from_static(crate::integration_test::AUTH_HEADER);
        let viewer_response = server
            .post("/users/authenticatebyname")
            .add_header(AUTHORIZATION, auth_header)
            .json(&json!({"Username":"viewer","Pw":"viewer"}))
            .await;
        let viewer: remux_sdks::remux::AuthenticationInfo = viewer_response.json();
        let viewer_header = crate::integration_test::auth_header_with_token(
            viewer
                .access_token
                .as_deref()
                .unwrap(),
        );
        server
            .get("/remux/logs")
            .add_header(
                AUTHORIZATION,
                HeaderValue::from_str(&viewer_header).unwrap(),
            )
            .expect_failure()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);

        let response = server
            .get("/remux/logs?level=warn&search=target&limit=1")
            .add_header(AUTHORIZATION, HeaderValue::from_str(&admin_header).unwrap())
            .await;
        let result: LogsResult = response.json();
        assert_eq!(
            result
                .entries
                .len(),
            1
        );
        assert_eq!(result.entries[0].level, "ERROR");
        assert_eq!(result.entries[0].timestamp, "2026-10-28 12:34:53");
        assert!(
            !result.entries[0]
                .message
                .contains("secret")
        );
    }

    #[tokio::test]
    async fn endpoint_defaults_to_bounded_pages_and_loads_older_entries() {
        let (server, _guard, temp, admin_token) = logs_server().await;
        let log_dir = temp
            .path()
            .join("logs");
        let content = (0..205)
            .map(|index| {
                format!(
                    "2026-10-28T12:34:{:02}+00:00 INFO remux: endpoint-{index}\n",
                    index % 60
                )
            })
            .collect::<String>();
        fs::write(log_dir.join("remux.2026-10-28.log"), content).unwrap();
        let admin_header =
            crate::integration_test::auth_header_with_token(&admin_token);

        let first_response = server
            .get("/remux/logs?level=all")
            .add_header(AUTHORIZATION, HeaderValue::from_str(&admin_header).unwrap())
            .await;
        first_response.assert_status_ok();
        let first: LogsResult = first_response.json();
        assert_eq!(
            first
                .entries
                .len(),
            200
        );
        assert_eq!(
            first
                .entries
                .first()
                .unwrap()
                .message,
            "endpoint-204"
        );
        assert_eq!(
            first
                .entries
                .last()
                .unwrap()
                .message,
            "endpoint-5"
        );
        assert!(first.has_more);

        let older_response = server
            .get("/remux/logs?level=all&limit=200&offset=200")
            .add_header(AUTHORIZATION, HeaderValue::from_str(&admin_header).unwrap())
            .await;
        older_response.assert_status_ok();
        let older: LogsResult = older_response.json();
        assert_eq!(
            older
                .entries
                .len(),
            5
        );
        assert_eq!(
            older
                .entries
                .first()
                .unwrap()
                .message,
            "endpoint-4"
        );
        assert_eq!(
            older
                .entries
                .last()
                .unwrap()
                .message,
            "endpoint-0"
        );
        assert!(!older.has_more);
    }
}
