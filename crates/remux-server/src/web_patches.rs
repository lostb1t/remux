/// CSS injected into `<head>` of every HTML response.
/// Targets stable `data-*` attributes and semantic class names rather than
/// minified JS internals, so it survives jellyfin-web bundle updates.
pub static CSS: &str = r##"
  /* ── Sidebar: whole sections ─────────────────────────────── */
  [aria-labelledby="plugins-subheader"]   { display: none !important; }

  /* ── Async media-sources spinner ─────────────────────────── */
  @keyframes remux-spin {
    to { transform: rotate(360deg); }
  }

  /* ── Play button: disabled by default, enabled when streams arrive ── */
  .detailPagePrimaryContainer .btnPlay {
    opacity: 0.4;
    pointer-events: none;
    cursor: default;
  }
  .detailPagePrimaryContainer.remux-streams-ready .btnPlay {
    opacity: 1;
    pointer-events: auto;
    cursor: pointer;
  }
"##;

/// JS injected before `</body>` of every HTML response.
/// Intercepts React Router (History API) navigation to /wizard and /dashboard
/// and redirects to our admin UI at /admin.
///
/// Kept in a separate file (rather than inline) so it can be loaded and
/// exercised directly by the jsdom test harness under `tests/web-patches-js/`.
pub static JS: &str = include_str!("../assets/web-patches.js");
