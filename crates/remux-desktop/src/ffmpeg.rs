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

fn platform_suffix() -> Option<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Some("osx-arm64");
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Some("osx-amd64");
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Some("linux-amd64");
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Some("linux-arm64");
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Some("win-x64");
    #[allow(unreachable_code)]
    None
}

pub fn ffmpeg_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("bin")
}

/// Ensure ffmpeg/ffprobe are present in `{data_dir}/bin/`, downloading
/// jellyfin-ffmpeg if needed. Sets FFMPEG_PATH and FFPROBE_PATH on success.
pub async fn ensure_ffmpeg(data_dir: &Path) -> Result<()> {
    let bin_dir = ffmpeg_dir(data_dir);
    let ffmpeg = bin_dir.join(FFMPEG_BIN);
    let ffprobe = bin_dir.join(FFPROBE_BIN);

    if ffmpeg.exists() && ffprobe.exists() {
        set_paths(&ffmpeg, &ffprobe);
        return Ok(());
    }

    tracing::info!("ffmpeg not found — downloading jellyfin-ffmpeg");
    std::fs::create_dir_all(&bin_dir)?;
    download(&bin_dir).await?;

    if !ffmpeg.exists() || !ffprobe.exists() {
        anyhow::bail!(
            "download succeeded but ffmpeg/ffprobe not found in {}",
            bin_dir.display()
        );
    }

    set_paths(&ffmpeg, &ffprobe);
    Ok(())
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

    let release: serde_json::Value = client
        .get("https://api.github.com/repos/jellyfin/jellyfin-ffmpeg/releases/latest")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no assets in release"))?;

    let asset = assets
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .map(|n| {
                    n.contains(suffix)
                        && (n.ends_with(".tar.gz") || n.ends_with(".zip"))
                })
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

    if name.ends_with(".tar.gz") {
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
