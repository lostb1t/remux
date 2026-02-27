/// CSS injected into `<head>` of every HTML response.
/// Targets stable `data-*` attributes and semantic class names rather than
/// minified JS internals, so it survives jellyfin-web bundle updates.
pub static CSS: &str = r##"
  /* ── Sidebar: whole sections ─────────────────────────────── */
  [aria-labelledby="livetv-subheader"]    { display: none !important; }
  [aria-labelledby="plugins-subheader"]   { display: none !important; }

  /* ── Sidebar: individual items ───────────────────────────── */
  li:has(a[href="#/dashboard/networking"])        { display: none !important; }
  li:has(a[href="#/dashboard/backups"])           { display: none !important; }
  li:has(a[href="#/dashboard/logs"])              { display: none !important; }
  li:has(a[href="#/dashboard/libraries/nfo"])     { display: none !important; }
  li:has(a[href="#/dashboard/libraries/display"]) { display: none !important; }
  li:has(a[href="#/dashboard/playback/trickplay"]) { display: none !important; }
"##;

/// JS injected before `</body>` of every HTML response.
/// Leave empty to skip injection entirely.
pub static JS: &str = r#"
(function () {
  var ADMIN = ['/wizardstart', '/startup/wizard', '/dashboard'];
  function checkHash() {
    var h = location.hash.replace(/^#\/?/, '');
    for (var i = 0; i < ADMIN.length; i++) {
      var p = ADMIN[i].slice(1);
      if (h === p || h.startsWith(p + '/') || h.startsWith(p + '?')) {
        location.replace('/admin');
        return;
      }
    }
  }
  checkHash();
  window.addEventListener('hashchange', checkHash);
}());
"#;
