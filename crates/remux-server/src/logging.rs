use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// Prepare the configured log directory without making file logging a startup
/// requirement. Returns `None` when file logging is disabled or unavailable.
pub fn prepare_log_dir(enabled: bool, log_dir: Option<&Path>) -> Option<PathBuf> {
    if !enabled {
        return None;
    }

    let Some(log_dir) = log_dir else {
        eprintln!(
            "Remux file logging unavailable: no log directory configured; continuing with stdout only"
        );
        return None;
    };

    match fs::create_dir_all(log_dir) {
        Ok(()) => Some(log_dir.to_path_buf()),
        Err(error) => {
            eprintln!(
                "Remux file logging unavailable for {}; continuing with stdout only: {error}",
                log_dir.display()
            );
            None
        }
    }
}

/// Remove daily logs older than the configured retention period. The rolling
/// appender independently enforces the configured maximum file count.
pub fn cleanup_old_logs(dir: &Path, retention_days: u32) {
    if retention_days == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    let now = SystemTime::now();
    let Some(cutoff) =
        now.checked_sub(Duration::from_secs(u64::from(retention_days) * 86_400))
    else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name == "remux.log"
            || (name.starts_with("remux.") && name.ends_with(".log")))
        {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH)
            < cutoff
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Periodically remove expired daily logs while the server runs.
pub fn start_periodic_cleanup(dir: PathBuf, retention_days: u32) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
            cleanup_old_logs(&dir, retention_days);
        }
    });
}

/// `path?query` for request logs, with the value of any credential-looking
/// parameter masked: Jellyfin clients put `ApiKey`/`api_key` in the URL.
pub fn loggable_uri(uri: &axum::http::Uri) -> String {
    let Some(query) = uri.query() else {
        return uri
            .path()
            .to_string();
    };
    let masked = url::form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| {
            let lower = key.to_ascii_lowercase();
            let secret = ["key", "token", "password", "secret", "auth"]
                .iter()
                .any(|needle| lower.contains(needle));
            let value = if secret { "<redacted>".into() } else { value };
            format!("{key}={value}")
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{masked}", uri.path())
}

#[cfg(test)]
mod tests {
    use super::{cleanup_old_logs, loggable_uri, prepare_log_dir};
    use std::{
        fs,
        time::{Duration, SystemTime},
    };

    #[test]
    fn file_logging_can_be_disabled_or_fall_back_when_directory_is_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp
            .path()
            .join("not-a-directory");
        fs::write(&file, "file").unwrap();
        assert!(prepare_log_dir(false, Some(temp.path())).is_none());
        assert!(prepare_log_dir(true, Some(&file)).is_none());
    }

    #[test]
    fn cleanup_removes_expired_files_without_enforcing_count_again() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let now = SystemTime::now();
        for (name, age_days) in [
            ("remux.2020-01-01.log", 10),
            ("remux.2026-10-26.log", 4),
            ("remux.2026-10-27.log", 3),
            ("remux.2026-10-28.log", 2),
        ] {
            let path = dir.join(name);
            fs::write(&path, name).unwrap();
            fs::File::open(&path)
                .unwrap()
                .set_times(
                    fs::FileTimes::new()
                        .set_modified(now - Duration::from_secs(age_days * 86_400)),
                )
                .unwrap();
        }
        let unrelated = dir.join("other.log");
        fs::write(&unrelated, "keep").unwrap();

        cleanup_old_logs(dir, 5);

        assert!(
            !dir.join("remux.2020-01-01.log")
                .exists()
        );
        assert!(
            dir.join("remux.2026-10-27.log")
                .exists()
        );
        assert!(
            dir.join("remux.2026-10-28.log")
                .exists()
        );
        assert!(
            dir.join("remux.2026-10-26.log")
                .exists()
        );
        assert!(unrelated.exists());
    }

    #[test]
    fn request_logs_mask_credential_query_parameters() {
        let uri: axum::http::Uri =
            "/items?ApiKey=abc&api_key=def&X-Emby-Token=ghi&limit=5&parentId=1"
                .parse()
                .unwrap();
        assert_eq!(
            loggable_uri(&uri),
            "/items?ApiKey=<redacted>&api_key=<redacted>&X-Emby-Token=<redacted>&limit=5&parentId=1"
        );
        let plain: axum::http::Uri = "/items"
            .parse()
            .unwrap();
        assert_eq!(loggable_uri(&plain), "/items");
    }
}
