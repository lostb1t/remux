//! Where the external tools the server runs (ffmpeg, ffprobe) live.
//!
//! A bootstrapping setting (see AGENTS.md): resolved from a path set at startup
//! (the `--ffmpeg`/`--ffprobe` flags, or the desktop app once it has fetched
//! ffmpeg), else `FFMPEG_PATH`/`FFPROBE_PATH`, else the bare name on `PATH`.
//! Setting a path here replaces mutating the process environment, which is
//! unsound once other threads run.

use std::sync::RwLock;

static FFMPEG: RwLock<Option<String>> = RwLock::new(None);
static FFPROBE: RwLock<Option<String>> = RwLock::new(None);

/// Use these binaries from now on.
pub fn set_paths(ffmpeg: Option<String>, ffprobe: Option<String>) {
    if let Some(path) = ffmpeg {
        *FFMPEG
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(path);
    }
    if let Some(path) = ffprobe {
        *FFPROBE
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(path);
    }
}

fn resolve(set: &RwLock<Option<String>>, env: &str, name: &str) -> String {
    set.read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .or_else(|| std::env::var(env).ok())
        .unwrap_or_else(|| name.to_string())
}

/// The ffmpeg binary.
pub fn ffmpeg() -> String {
    resolve(&FFMPEG, "FFMPEG_PATH", "ffmpeg")
}

/// The ffprobe binary.
pub fn ffprobe() -> String {
    resolve(&FFPROBE, "FFPROBE_PATH", "ffprobe")
}
