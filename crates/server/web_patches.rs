/// CSS injected into `<head>` of every HTML response.
/// Targets stable `data-*` attributes and semantic class names rather than
/// minified JS internals, so it survives jellyfin-web bundle updates.
pub static CSS: &str = r##"
  /* ── Sidebar: whole sections ─────────────────────────────── */
  [aria-labelledby="livetv-subheader"]    { display: none !important; }
  [aria-labelledby="plugins-subheader"]   { display: none !important; }
"##;

/// JS injected before `</body>` of every HTML response.
/// Intercepts React Router (History API) navigation to /wizard and /dashboard
/// and redirects to our admin UI at /admin.
pub static JS: &str = r#"
(function () {
  var ADMIN = ['/wizard', '/dashboard'];

  function matchesAdmin(p) {
    for (var i = 0; i < ADMIN.length; i++) {
      var a = ADMIN[i];
      if (p === a || p.startsWith(a + '/') || p.startsWith(a + '?')) return true;
    }
    return false;
  }

  // Check both pathname and hash (createHashRouter stores route in hash)
  function checkUrl(url) {
    try {
      var u = new URL(String(url), location.href);
      if (matchesAdmin(u.pathname)) { location.replace('/admin'); return true; }
      if (u.hash) {
        var h = '/' + u.hash.replace(/^#\/?/, '');
        if (matchesAdmin(h)) { location.replace('/admin'); return true; }
      }
    } catch(e) {}
    return false;
  }

  function checkCurrent() {
    return checkUrl(location.href);
  }

  if (checkCurrent()) return;

  // Intercept React Router History API (covers both BrowserRouter and HashRouter)
  var _push = history.pushState.bind(history);
  var _replace = history.replaceState.bind(history);
  history.pushState = function(s, t, url) {
    if (url && checkUrl(url)) return;
    return _push(s, t, url);
  };
  history.replaceState = function(s, t, url) {
    if (url && checkUrl(url)) return;
    return _replace(s, t, url);
  };
  window.addEventListener('popstate', checkCurrent);
  window.addEventListener('hashchange', checkCurrent);
}());
"#;
