/// CSS injected into `<head>` of every HTML response.
/// Targets stable `data-*` attributes and semantic class names rather than
/// minified JS internals, so it survives jellyfin-web bundle updates.
pub static CSS: &str = r#"
  /* ── Sidebar ─────────────────────────────────────────────── */
  /* Hide the entire Live TV section (header + all links beneath it) */
  [aria-labelledby="livetv-subheader"] { display: none !important; }
"#;

/// JS injected before `</body>` of every HTML response.
/// Leave empty to skip injection entirely.
pub static JS: &str = "";
