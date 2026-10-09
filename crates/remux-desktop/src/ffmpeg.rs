use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
const FFMPEG_BIN: &str = "ffmpeg.exe";
#[cfg(not(target_os = "windows"))]
const FFMPEG_BIN: &str = "ffmpeg";

#[cfg(target_os = "windows")]
const FFPROBE_BIN: &str = "ffprobe.exe";
#[cfg(not(target_os = "windows"))]
const FFPROBE_BIN: &str = "ffprobe";

/// Major jellyfin-ffmpeg version the app downloads and expects. Newer majors
/// are only picked up deliberately, by changing this.
const FFMPEG_MAJOR: u32 = 8;

fn platform_suffix() -> Option<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Some("macarm64");
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Some("mac64");
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Some("linux64");
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Some("linuxarm64");
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Some("win64");
    #[allow(unreachable_code)]
    None
}

pub fn ffmpeg_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("bin")
}

/// Ensure ffmpeg/ffprobe from jellyfin-ffmpeg `FFMPEG_MAJOR` are present in
/// `{data_dir}/bin/`, downloading (or upgrading) them if needed. Sets
/// FFMPEG_PATH and FFPROBE_PATH on success.
pub async fn ensure_ffmpeg(data_dir: &Path) -> Result<()> {
    let bin_dir = ffmpeg_dir(data_dir);
    let ffmpeg = bin_dir.join(FFMPEG_BIN);
    let ffprobe = bin_dir.join(FFPROBE_BIN);

    let installed = ffmpeg.exists() && ffprobe.exists();
    if installed && installed_major(&ffmpeg).await == Some(FFMPEG_MAJOR) {
        set_paths(&ffmpeg, &ffprobe);
        return Ok(());
    }

    if installed {
        tracing::info!(
            "installed ffmpeg is not jellyfin-ffmpeg {FFMPEG_MAJOR} — upgrading"
        );
    } else {
        tracing::info!("ffmpeg not found — downloading jellyfin-ffmpeg");
    }
    std::fs::create_dir_all(&bin_dir)?;

    if let Err(e) = download(&bin_dir).await {
        tracing::warn!("jellyfin-ffmpeg download failed: {e:#}");
        if installed {
            // An older ffmpeg still transcodes; don't leave the user without one.
            tracing::warn!("keeping the previously installed ffmpeg");
            set_paths(&ffmpeg, &ffprobe);
            return Ok(());
        }
        if let Some((ff, ffp)) = system_ffmpeg() {
            tracing::info!(ffmpeg = %ff.display(), "falling back to system ffmpeg");
            set_paths(&ff, &ffp);
            return Ok(());
        }
        return Err(e);
    }

    if !ffmpeg.exists() || !ffprobe.exists() {
        anyhow::bail!(
            "download succeeded but ffmpeg/ffprobe not found in {}",
            bin_dir.display()
        );
    }

    set_paths(&ffmpeg, &ffprobe);
    Ok(())
}

/// Major version reported by `ffmpeg -version`, or `None` if it can't be run
/// or parsed (treated as stale, so it gets replaced).
async fn installed_major(ffmpeg: &Path) -> Option<u32> {
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.arg("-version")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), cmd.output())
        .await
        .ok()?
        .ok()?;
    parse_ffmpeg_major(&String::from_utf8_lossy(&output.stdout))
}

/// Parses `ffmpeg version 8.1.3-Jellyfin ...` (or a git tag like `n7.1`) to
/// its major version. Snapshot builds (`N-12345-g...`) have none.
fn parse_ffmpeg_major(version_output: &str) -> Option<u32> {
    let token = version_output
        .lines()
        .next()?
        .split_whitespace()
        .skip_while(|word| *word != "version")
        .nth(1)?;
    let token = token
        .strip_prefix('n')
        .unwrap_or(token);
    let digits: String = token
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() || !token[digits.len()..].starts_with('.') {
        return None;
    }
    digits
        .parse()
        .ok()
}

/// Newest stable release (GitHub lists newest first) whose tag is `v{major}.…`.
fn pick_release(
    releases: &[serde_json::Value],
    major: u32,
) -> Option<&serde_json::Value> {
    let prefix = format!("v{major}.");
    releases
        .iter()
        .find(|release| {
            !release["draft"]
                .as_bool()
                .unwrap_or(false)
                && !release["prerelease"]
                    .as_bool()
                    .unwrap_or(false)
                && release["tag_name"]
                    .as_str()
                    .is_some_and(|tag| tag.starts_with(&prefix))
        })
}

fn system_ffmpeg() -> Option<(PathBuf, PathBuf)> {
    let dirs: &[&str] = &[
        #[cfg(target_os = "macos")]
        "/opt/homebrew/bin",
        #[cfg(target_os = "macos")]
        "/usr/local/bin",
        "/usr/bin",
        "/usr/local/bin",
    ];
    for dir in dirs {
        let ff = PathBuf::from(dir).join(FFMPEG_BIN);
        let ffp = PathBuf::from(dir).join(FFPROBE_BIN);
        if ff.exists() && ffp.exists() {
            return Some((ff, ffp));
        }
    }
    None
}

fn set_paths(ffmpeg: &Path, ffprobe: &Path) {
    unsafe {
        std::env::set_var("FFMPEG_PATH", ffmpeg);
        std::env::set_var("FFPROBE_PATH", ffprobe);
    }
    tracing::info!(
        ffmpeg = %ffmpeg.display(),
        ffprobe = %ffprobe.display(),
        "ffmpeg paths set"
    );
}

async fn download(bin_dir: &Path) -> Result<()> {
    let suffix = platform_suffix()
        .ok_or_else(|| anyhow::anyhow!("unsupported platform for ffmpeg download"))?;

    let client = reqwest::Client::builder()
        .user_agent("remux-desktop")
        .build()?;

    let releases: Vec<serde_json::Value> = client
        .get("https://api.github.com/repos/jellyfin/jellyfin-ffmpeg/releases?per_page=50")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let release = pick_release(&releases, FFMPEG_MAJOR).ok_or_else(|| {
        anyhow::anyhow!("no jellyfin-ffmpeg {FFMPEG_MAJOR}.x release found")
    })?;

    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no assets in release"))?;

    let asset = assets
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .map(|n| n.contains(suffix) && n.contains("portable"))
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            anyhow::anyhow!("no jellyfin-ffmpeg asset for platform '{suffix}'")
        })?;

    let url = asset["browser_download_url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing download URL"))?;
    let name = asset["name"]
        .as_str()
        .unwrap_or("");

    tracing::info!(url, "downloading jellyfin-ffmpeg");
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    if name.ends_with(".tar.xz") {
        extract_tar_xz(&bytes, bin_dir)?;
    } else if name.ends_with(".tar.gz") {
        extract_tar_gz(&bytes, bin_dir)?;
    } else if name.ends_with(".zip") {
        extract_zip(&bytes, bin_dir)?;
    } else {
        bail!("unknown archive format: {name}");
    }

    #[cfg(unix)]
    set_executable(bin_dir)?;

    tracing::info!(dir = %bin_dir.display(), "jellyfin-ffmpeg installed");
    Ok(())
}

fn extract_tar_xz(data: &[u8], dest: &Path) -> Result<()> {
    use tar::Archive;
    use xz2::read::XzDecoder;

    let mut archive = Archive::new(XzDecoder::new(data));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if let Some(name) = path.file_name() {
            if name == "ffmpeg" || name == "ffprobe" {
                entry.unpack(dest.join(name))?;
            }
        }
    }
    Ok(())
}

fn extract_tar_gz(data: &[u8], dest: &Path) -> Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let mut archive = Archive::new(GzDecoder::new(data));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if let Some(name) = path.file_name() {
            if name == "ffmpeg" || name == "ffprobe" {
                entry.unpack(dest.join(name))?;
            }
        }
    }
    Ok(())
}

fn extract_zip(data: &[u8], dest: &Path) -> Result<()> {
    use std::io::Cursor;

    let mut zip = zip::ZipArchive::new(Cursor::new(data))?;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let raw_name = file
            .name()
            .to_string();
        let file_name = Path::new(&raw_name)
            .file_name()
            .map(|n| {
                n.to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default();
        if file_name == "ffmpeg.exe" || file_name == "ffprobe.exe" {
            let out = dest.join(&file_name);
            let mut out_file = std::fs::File::create(&out)?;
            std::io::copy(&mut file, &mut out_file)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(bin_dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for name in [FFMPEG_BIN, FFPROBE_BIN] {
        let path = bin_dir.join(name);
        if path.exists() {
            let mut perms = std::fs::metadata(&path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_the_major_version_from_ffmpeg_output() {
        let major = |s: &str| parse_ffmpeg_major(s);
        assert_eq!(
            major(
                "ffmpeg version 8.1.3-Jellyfin Copyright (c) 2000-2026 the FFmpeg developers\nbuilt with clang"
            ),
            Some(8)
        );
        assert_eq!(major("ffmpeg version 7.1.3-Jellyfin Copyright"), Some(7));
        assert_eq!(major("ffmpeg version n6.0.1 Copyright"), Some(6));
        assert_eq!(major("ffmpeg version 10.0 Copyright"), Some(10));
        // Snapshot builds carry no release number.
        assert_eq!(major("ffmpeg version N-117534-g1234abc Copyright"), None);
        assert_eq!(major("ffmpeg version 8 Copyright"), None);
        assert_eq!(major("not ffmpeg at all"), None);
        assert_eq!(major(""), None);
    }

    #[test]
    fn picks_the_newest_stable_release_of_the_pinned_major() {
        let releases = vec![
            json!({"tag_name": "v9.0.0-1", "draft": false, "prerelease": false}),
            json!({"tag_name": "v8.2.0-1", "draft": false, "prerelease": true}),
            json!({"tag_name": "v8.1.3-1", "draft": true, "prerelease": false}),
            json!({"tag_name": "v8.1.2-5", "draft": false, "prerelease": false}),
            json!({"tag_name": "v8.1.2-4", "draft": false, "prerelease": false}),
            json!({"tag_name": "v7.1.4-3", "draft": false, "prerelease": false}),
        ];
        let picked = pick_release(&releases, 8).expect("a v8 release");
        assert_eq!(picked["tag_name"], "v8.1.2-5");
        assert_eq!(pick_release(&releases, 7).unwrap()["tag_name"], "v7.1.4-3");
        assert!(pick_release(&releases, 6).is_none());
        // `v8.` must not match `v80.` style tags.
        assert!(pick_release(&[json!({"tag_name": "v80.1-1"})], 8).is_none());
    }
}
