use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use remux_macros::{get, post};
use serde::Deserialize;
use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use crate::AppState;
use crate::api;
use crate::db::auth;
use anyhow;
use axum_anyhow::{ApiResult as Result, ResultExt};

const ANFITEATRO_REPO_OWNER: &str = "j4ckgrey";
const ANFITEATRO_REPO_NAME: &str = "Anfiteatro_web";
const ANFITEATRO_REPO_BRANCH: &str = "main";
const GITHUB_API_ACCEPT: &str = "application/vnd.github+json";
const ANFITEATRO_COMMIT_MARKER: &str = ".remux-anfiteatro-commit";

#[derive(Debug, Deserialize)]
struct GitHubBranch {
    name: String,
    commit: GitHubBranchCommit,
}

#[derive(Debug, Deserialize)]
struct GitHubBranchCommit {
    sha: String,
    html_url: Option<String>,
}

#[derive(Debug, Clone)]
struct LatestAnfiteatroHead {
    branch: String,
    commit_sha: String,
    commit_url: String,
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn configured_anfiteatro_path(state: &AppState) -> Option<String> {
    state.ctx.web_paths.as_ref().map(|p| p.anfiteatro_web_path.clone())
}

fn commit_marker_path(path: &Path) -> std::path::PathBuf {
    path.join(ANFITEATRO_COMMIT_MARKER)
}

fn normalize_commit_sha(raw: &str) -> Option<String> {
    let sha = raw.trim();
    if sha.len() >= 7 && sha.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(sha.to_ascii_lowercase())
    } else {
        None
    }
}

fn read_local_commit_marker(path: &str) -> Option<String> {
    let marker_path = commit_marker_path(Path::new(path));
    let raw = std::fs::read_to_string(marker_path).ok()?;
    normalize_commit_sha(&raw)
}

fn write_local_commit_marker(path: &Path, commit_sha: &str) -> std::result::Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|err| format!("failed to create {}: {err}", path.display()))?;
    std::fs::write(commit_marker_path(path), format!("{commit_sha}\n"))
        .map_err(|err| format!("failed to write commit marker in {}: {err}", path.display()))
}

fn clear_directory_contents(path: &Path) -> std::result::Result<(), String> {
    if !path.exists() {
        std::fs::create_dir_all(path)
            .map_err(|err| format!("failed to create {}: {err}", path.display()))?;
        return Ok(());
    }

    let entries = std::fs::read_dir(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;

    for entry in entries {
        let entry =
            entry.map_err(|err| format!("failed to read entry in {}: {err}", path.display()))?;
        let entry_path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| format!("failed to inspect {}: {err}", entry_path.display()))?;

        if file_type.is_dir() {
            std::fs::remove_dir_all(&entry_path)
                .map_err(|err| format!("failed to remove {}: {err}", entry_path.display()))?;
        } else {
            std::fs::remove_file(&entry_path)
                .map_err(|err| format!("failed to remove {}: {err}", entry_path.display()))?;
        }
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::result::Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|err| format!("failed to create {}: {err}", dst.display()))?;

    let entries =
        std::fs::read_dir(src).map_err(|err| format!("failed to read {}: {err}", src.display()))?;

    for entry in entries {
        let entry =
            entry.map_err(|err| format!("failed to read entry in {}: {err}", src.display()))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|err| format!("failed to inspect {}: {err}", src_path.display()))?;

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&src_path, &dst_path).map_err(|err| {
                format!(
                    "failed to copy {} to {}: {err}",
                    src_path.display(),
                    dst_path.display()
                )
            })?;
        }
    }

    Ok(())
}

async fn download_anfiteatro_archive(
    client: &reqwest::Client,
    commit_sha: &str,
) -> std::result::Result<Vec<u8>, String> {
    let tarball_url = format!(
        "https://api.github.com/repos/{}/{}/tarball/{}",
        ANFITEATRO_REPO_OWNER, ANFITEATRO_REPO_NAME, commit_sha
    );

    let bytes = client
        .get(&tarball_url)
        .header(reqwest::header::USER_AGENT, "remux-server")
        .header(reqwest::header::ACCEPT, GITHUB_API_ACCEPT)
        .send()
        .await
        .map_err(|err| format!("archive request failed: {err}"))?
        .error_for_status()
        .map_err(|err| format!("archive request failed: {err}"))?
        .bytes()
        .await
        .map_err(|err| format!("failed to download archive bytes: {err}"))?;

    Ok(bytes.to_vec())
}

fn install_archive_to_path(
    archive_bytes: &[u8],
    target_path: &Path,
) -> std::result::Result<(), String> {
    let temp_dir = tempfile::tempdir().map_err(|err| format!("failed to create temp dir: {err}"))?;
    let decoder = flate2::read::GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = tar::Archive::new(decoder);

    archive
        .unpack(temp_dir.path())
        .map_err(|err| format!("failed to unpack Anfiteatro archive: {err}"))?;

    let extracted_root = std::fs::read_dir(temp_dir.path())
        .map_err(|err| {
            format!(
                "failed to read extracted archive root {}: {err}",
                temp_dir.path().display()
            )
        })?
        .filter_map(std::result::Result::ok)
        .find_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|ft| ft.is_dir())
                .map(|_| entry.path())
        })
        .ok_or_else(|| "archive unpack did not produce a root directory".to_string())?;

    clear_directory_contents(target_path)?;
    copy_dir_recursive(&extracted_root, target_path)?;
    Ok(())
}

fn local_anfiteatro_commit(path: &str) -> Option<String> {
    read_local_commit_marker(path)
}

async fn fetch_latest_anfiteatro_head(
    client: &reqwest::Client,
) -> std::result::Result<LatestAnfiteatroHead, String> {
    let branch_url = format!(
        "https://api.github.com/repos/{}/{}/branches/{}",
        ANFITEATRO_REPO_OWNER, ANFITEATRO_REPO_NAME, ANFITEATRO_REPO_BRANCH
    );

    let branch = client
        .get(&branch_url)
        .header(reqwest::header::USER_AGENT, "remux-server")
        .header(reqwest::header::ACCEPT, GITHUB_API_ACCEPT)
        .send()
        .await
        .map_err(|err| format!("branch request failed: {err}"))?
        .error_for_status()
        .map_err(|err| format!("branch request failed: {err}"))?
        .json::<GitHubBranch>()
        .await
        .map_err(|err| format!("invalid GitHub branch payload: {err}"))?;

    let commit_sha = branch.commit.sha;
    let commit_url = branch.commit.html_url.unwrap_or_else(|| {
        format!(
            "https://github.com/{}/{}/commit/{}",
            ANFITEATRO_REPO_OWNER, ANFITEATRO_REPO_NAME, commit_sha
        )
    });

    Ok(LatestAnfiteatroHead {
        branch: branch.name,
        commit_sha,
        commit_url,
    })
}

#[get("/admin/clients/anfiteatro/release")]
pub async fn anfiteatro_release_status(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    let Some(target_path) = configured_anfiteatro_path(&state) else {
        let mut status = api::AnfiteatroReleaseStatus::default();
        status.check_error =
            Some("Anfiteatro release checks are unavailable in desktop builds".to_string());
        return Ok(Json(status));
    };

    let local_commit = local_anfiteatro_commit(&target_path);
    let mut status = api::AnfiteatroReleaseStatus {
        local_version_display: local_commit
            .as_deref()
            .map(|sha| format!("commit {}", short_sha(sha))),
        local_commit: local_commit.clone(),
        ..Default::default()
    };

    let client = match reqwest::Client::builder().timeout(Duration::from_secs(8)).build() {
        Ok(client) => client,
        Err(err) => {
            status.check_error = Some(format!("failed to build HTTP client: {err}"));
            return Ok(Json(status));
        }
    };

    match fetch_latest_anfiteatro_head(&client).await {
        Ok(latest) => {
            status.latest_version_tag =
                Some(format!("{}@{}", latest.branch, short_sha(&latest.commit_sha)));
            status.latest_release_url = Some(latest.commit_url);
            status.latest_commit = Some(latest.commit_sha.clone());
            status.update_available = match local_commit.as_deref() {
                Some(local) => !local.eq_ignore_ascii_case(&latest.commit_sha),
                None => true,
            };
        }
        Err(err) => {
            status.check_error = Some(err);
        }
    }

    if let Some(err) = status.check_error.as_deref() {
        tracing::warn!("Anfiteatro commit check completed with warning: {}", err);
    }

    Ok(Json(status))
}

#[post("/admin/clients/anfiteatro/release/install")]
pub async fn install_latest_anfiteatro_release(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    let target_path = configured_anfiteatro_path(&state)
        .ok_or_else(|| anyhow::anyhow!("no Anfiteatro web path configured"))
        .context_bad_request(
            "Install failed",
            "Anfiteatro install is unavailable in desktop builds",
        )?;
    let before_commit = local_anfiteatro_commit(&target_path);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|err| anyhow::anyhow!("failed to build HTTP client: {err}"))?;

    let latest = fetch_latest_anfiteatro_head(&client)
        .await
        .map_err(anyhow::Error::msg)?;

    let repo_path = Path::new(&target_path);
    let archive_bytes = download_anfiteatro_archive(&client, &latest.commit_sha)
        .await
        .map_err(anyhow::Error::msg)?;
    install_archive_to_path(&archive_bytes, repo_path).map_err(anyhow::Error::msg)?;

    write_local_commit_marker(repo_path, &latest.commit_sha).map_err(anyhow::Error::msg)?;

    let after_commit = local_anfiteatro_commit(&target_path);
    let changed = before_commit
        .as_deref()
        .map(|local| !local.eq_ignore_ascii_case(&latest.commit_sha))
        .unwrap_or(true);

    Ok(Json(api::AnfiteatroInstallResult {
        installed_tag: Some(format!("{}@{}", latest.branch, short_sha(&latest.commit_sha))),
        installed_commit: after_commit.clone(),
        local_version_display: after_commit
            .as_deref()
            .map(|sha| format!("commit {}", short_sha(sha))),
        changed,
        message: format!(
            "Installed Anfiteatro commit {} from {}{}",
            short_sha(&latest.commit_sha),
            latest.branch,
            if changed { "" } else { " (already up to date)" }
        ),
    }))
}