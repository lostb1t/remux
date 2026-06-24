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

(function () {
  var _get = Storage.prototype.getItem;
  Storage.prototype.getItem = function (key) {
    var val = _get.call(this, key);
    if (typeof key === 'string' && /maxbitrate-video-false/i.test(key) && (val === null || val === '15000')) {
      return '0';
    }
    return val;
  };
}());

// Async MediaSources loader for the item details page.
// Patches ApiClient.prototype.getItem (available via window.ApiClient) to skip
// stream loading on the initial fetch (Fields=ChildCount), making the server
// respond faster. For Movie/Episode a spinner appears while a second getItem call
// retrieves MediaSources and populates the track-selection UI.
(function () {
  var _videoNavCount = 0; // increments each time a video item page is entered

  function escHtml(s) {
    return String(s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function getDetailsPage() {
    // Jellyfin caches views: multiple .trackSelections may exist in the DOM
    // (one per cached view). Use offsetParent to find the visible one.
    var all = document.querySelectorAll('.trackSelections');
    for (var i = 0; i < all.length; i++) {
      if (all[i].offsetParent !== null) return all[i].closest('.detailPagePrimaryContent');
    }
    return null;
  }

  function getVisiblePrimaryContainer() {
    var all = document.querySelectorAll('.detailPagePrimaryContainer');
    for (var i = 0; i < all.length; i++) {
      if (all[i].offsetParent !== null) return all[i];
    }
    return null;
  }

  function showSpinner(page) {
    removeSpinner(page);
    var form = page.querySelector('.trackSelections');
    if (!form) return;
    var spin = document.createElement('div');
    spin.className = 'remux-sources-loading';
    // margin:auto centres the item in any flex or block context the theme uses
    spin.style.cssText = 'width:1.4em;height:1.4em;border:2px solid rgba(255,255,255,0.2);border-top-color:rgba(255,255,255,0.8);border-radius:50%;animation:remux-spin 0.7s linear infinite;margin:0.4em auto;display:block;flex-shrink:0;';
    form.insertBefore(spin, form.firstChild);
    form.classList.remove('hide');
  }

  function removeSpinner(page) {
    var el = page.querySelector('.remux-sources-loading');
    if (el && el.parentNode) el.parentNode.removeChild(el);
    var noStreams = page.querySelector('.remux-no-streams');
    if (noStreams && noStreams.parentNode) noStreams.parentNode.removeChild(noStreams);
    // re-hide the form if sources haven't arrived yet
    var form = page.querySelector('.trackSelections');
    if (form && !form._remuxLoaded) form.classList.add('hide');
  }

  function showNoStreams(page) {
    removeSpinner(page);
    var form = page.querySelector('.trackSelections');
    if (!form) return;
    var msg = document.createElement('div');
    msg.className = 'remux-no-streams';
    msg.style.cssText = 'color:rgba(255,255,255,0.5);font-size:0.85em;text-align:center;padding:0.4em 0;';
    msg.textContent = 'No streams available';
    form.insertBefore(msg, form.firstChild);
    form.classList.remove('hide');
  }

  function disablePlayButton(page) {
    var container = page && page.closest('.detailPagePrimaryContainer');
    if (container) container.classList.remove('remux-streams-ready');
  }

  function enablePlayButton(page) {
    var container = page && page.closest('.detailPagePrimaryContainer');
    if (container) container.classList.add('remux-streams-ready');
  }

  function renderTracksForSource(page, mediaSources, selectedSourceId) {
    var source = null;
    for (var i = 0; i < mediaSources.length; i++) {
      if (mediaSources[i].Id === selectedSourceId) { source = mediaSources[i]; break; }
    }
    if (!source) source = mediaSources[0];
    var streams = source.MediaStreams || [];

    // Video — display-only, always disabled
    var videoTracks = streams.filter(function (s) { return s.Type === 'Video'; });
    var selVideo = page.querySelector('.selectVideo');
    if (selVideo.setLabel) selVideo.setLabel('Video');
    selVideo.innerHTML = videoTracks.map(function (v) {
      return '<option value="' + v.Index + '" selected>' + escHtml(v.DisplayTitle || v.Codec || String(v.Index)) + '</option>';
    }).join('');
    selVideo.setAttribute('disabled', 'disabled');
    page.querySelector('.selectVideoContainer').classList[videoTracks.length ? 'remove' : 'add']('hide');

    // Audio
    var audioTracks = streams.filter(function (s) { return s.Type === 'Audio'; });
    var selAudio = page.querySelector('.selectAudio');
    if (selAudio.setLabel) selAudio.setLabel('Audio');
    var defAudio = source.DefaultAudioStreamIndex;
    selAudio.innerHTML = audioTracks.map(function (v) {
      var sel = v.Index === defAudio ? ' selected' : '';
      return '<option value="' + v.Index + '"' + sel + '>' + escHtml(v.DisplayTitle || String(v.Index)) + '</option>';
    }).join('');
    selAudio[audioTracks.length > 1 ? 'removeAttribute' : 'setAttribute']('disabled', 'disabled');
    page.querySelector('.selectAudioContainer').classList[audioTracks.length ? 'remove' : 'add']('hide');

    // Subtitles
    var subTracks = streams.filter(function (s) { return s.Type === 'Subtitle'; });
    var selSubs = page.querySelector('.selectSubtitles');
    if (selSubs.setLabel) selSubs.setLabel('Subtitles');
    var defSub = source.DefaultSubtitleStreamIndex == null ? -1 : source.DefaultSubtitleStreamIndex;
    var offSel = defSub === -1 ? ' selected' : '';
    selSubs.innerHTML = '<option value="-1"' + offSel + '>Off</option>' + subTracks.map(function (v) {
      var sel = v.Index === defSub ? ' selected' : '';
      return '<option value="' + v.Index + '"' + sel + '>' + escHtml(v.DisplayTitle || String(v.Index)) + '</option>';
    }).join('');
    selSubs[subTracks.length ? 'removeAttribute' : 'setAttribute']('disabled', 'disabled');
    page.querySelector('.selectSubtitlesContainer').classList[subTracks.length ? 'remove' : 'add']('hide');
  }

  function renderAsyncTrackSelections(page, mediaSources) {
    var form = page.querySelector('.trackSelections');
    if (!form) return;

    var selSrc = page.querySelector('.selectSource');
    var selectedId = mediaSources[0].Id;
    selSrc.innerHTML = mediaSources.map(function (v) {
      var sel = v.Id === selectedId ? ' selected' : '';
      return '<option value="' + escHtml(v.Id) + '"' + sel + '>' + escHtml(v.Name) + '</option>';
    }).join('');
    if (selSrc.setLabel) selSrc.setLabel('Version');
    page.querySelector('.selectSourceContainer').classList[mediaSources.length > 1 ? 'remove' : 'add']('hide');

    renderTracksForSource(page, mediaSources, selectedId);

    window._remuxCurrentMediaSources = mediaSources;
    form._remuxMediaSources = mediaSources;
    form._remuxLoaded = true;

    // Hide the whole panel when there are no meaningful choices:
    // single version, at most one audio track, and no subtitles.
    var source = mediaSources[0];
    var streams = source.MediaStreams || [];
    var hasChoice = mediaSources.length > 1
      || streams.filter(function (s) { return s.Type === 'Audio'; }).length > 1
      || streams.some(function (s) { return s.Type === 'Subtitle'; });
    if (hasChoice) {
      form.classList.remove('hide');
    } else {
      form.classList.add('hide');
    }
  }

  // Adds a second change listener that re-renders stream dropdowns when the user picks
  // a different version. The original listener throws because self._currentPlaybackMediaSources
  // is null (renderTrackSelections was called without MediaSources), but our listener runs
  // after the throw and renders correctly from window._remuxCurrentMediaSources.
  function attachSourceChangeHandler(page) {
    var sel = page.querySelector('.selectSource');
    if (sel._remuxHandlerAttached) return;
    sel._remuxHandlerAttached = true;
    sel.addEventListener('change', function () {
      var ms = window._remuxCurrentMediaSources;
      if (!ms) return;
      renderTracksForSource(page, ms, sel.value);
    });
  }

  // Matches both /Users/{uuid}/Items/{uuid} and /Items/{uuid} (no trailing path).
  var ITEM_PATH_RE = /\/(Users\/[0-9a-f-]{36}\/)?Items\/[0-9a-f-]{36}$/i;

  function patchApiClientProto(apiClient) {
    var proto = Object.getPrototypeOf(apiClient);
    if (!proto || proto._remuxGetItemPatched) return;
    proto._remuxGetItemPatched = true;

    // Patch the apiclient's own fetch class method to catch any call to the
    // single-item endpoint that bypasses getItem (e.g. direct getJSON/ajax calls).
    var _origApiFetch = proto.fetch;
    proto.fetch = function (opts, b) {
      if (opts && opts.url && (!opts.type || opts.type === 'GET')) {
        try {
          var pu = new URL(opts.url);
          if (ITEM_PATH_RE.test(pu.pathname) && !pu.searchParams.has('Fields')) {
            pu.searchParams.set('Fields', 'ChildCount');
            opts = Object.assign({}, opts, { url: pu.toString() });
          }
        } catch (ex) {}
      }
      return _origApiFetch.call(this, opts, b);
    };

    proto.getItem = function (userId, itemId) {
      var self = this;
      var capturedId = itemId;
      // Strip dashes so we can match against both UUID formats in the URL.
      var capturedIdNoDash = itemId.replace(/-/g, '');
      var baseUrl = self.getUrl('Users/' + userId + '/Items/' + itemId);
      var sep = baseUrl.indexOf('?') >= 0 ? '&' : '?';
      var fastUrl = baseUrl + sep + 'Fields=ChildCount';

      // True when the current URL belongs to this item's detail page.
      // Related-item fetches (next-up cards, season metadata, previews) have IDs
      // that do not appear in the URL, so this returns false for them.
      function isCurrentPage() {
        var h = location.href;
        return h.indexOf(capturedId) !== -1 || h.indexOf(capturedIdNoDash) !== -1;
      }

      return self.getJSON(fastUrl).then(function (item) {
        var type = item && item.Type;
        var isMovieOrEpisode = (type === 'Movie' || type === 'Episode');
        // Skip everything for related-item fetches — only process the item whose
        // ID is reflected in the current page URL.
        if (!isCurrentPage()) return item;

        // Shared helper: enable the play button on the VISIBLE primary container.
        // Jellyfin caches old views in the DOM (hidden), so querySelector('.detailPagePrimaryContainer')
        // may return a hidden old view's container. We use offsetParent to find the visible one.
        function watchAndEnable() {
          var seen = new WeakSet();
          function tryEnable() {
            if (!isCurrentPage()) { wObs.disconnect(); return; }
            var c = getVisiblePrimaryContainer();
            if (c && !seen.has(c)) { seen.add(c); c.classList.add('remux-streams-ready'); }
          }
          var wObs = new MutationObserver(function () { tryEnable(); });
          wObs.observe(document.body, { childList: true, subtree: true });
          tryEnable();
          setTimeout(function () { wObs.disconnect(); }, 5000);
        }

        if (!isMovieOrEpisode) {
          // Non-video (Series, Season, etc.): enable play button immediately.
          watchAndEnable();
        } else {
          // Video (Movie/Episode): load streams, then enable play button.
          var capturedNav = ++_videoNavCount;
          var sourcesUrl = baseUrl + sep + 'Fields=MediaSources';
          var sourcesFetch = self.getJSON(sourcesUrl);

          setTimeout(function () {
            if (!isCurrentPage()) return;
            var page = getDetailsPage();
            if (!page) return;
            var form = page.querySelector('.trackSelections');
            if (form && form._remuxNavCount === capturedNav) return;
            showSpinner(page);
          }, 0);

          sourcesFetch.then(function (full) {
            if (!isCurrentPage()) return;
            var ms = full && full.MediaSources;
            var streamsReady = ms && ms.length && full.LocationType !== 'Virtual';

            if (streamsReady) {
              // Enable the play button as soon as streams are confirmed.
              watchAndEnable();
            }

            // Best-effort: render the audio/subtitle track selector UI.
            (function apply() {
              if (!isCurrentPage()) return;
              var page = getDetailsPage();
              if (!page) { setTimeout(apply, 50); return; }
              var form = page.querySelector('.trackSelections');
              if (form && form._remuxNavCount === capturedNav) return;
              removeSpinner(page);
              if (streamsReady) {
                renderAsyncTrackSelections(page, ms);
                attachSourceChangeHandler(page);
                var f = page.querySelector('.trackSelections');
                if (f) f._remuxNavCount = capturedNav;
              } else {
                showNoStreams(page);
              }
            }());
          }).catch(function () {
            if (!isCurrentPage()) return;
            var page = getDetailsPage();
            if (page) {
              var form = page.querySelector('.trackSelections');
              if (!form || !form._remuxLoaded) removeSpinner(page);
            }
          });
        }

        return item;
      });
    };
  }

  // Intercept the exact moment window.ApiClient is assigned by ServerConnections.
  // This runs synchronously before any defer scripts, so the trap is in place
  // before the app initialises. No polling needed.
  var _realApiClient = null;
  try {
    Object.defineProperty(window, 'ApiClient', {
      configurable: true,
      get: function () { return _realApiClient; },
      set: function (v) {
        _realApiClient = v;
        if (v) patchApiClientProto(v);
      }
    });
  } catch (e) {
    // Fallback if defineProperty fails (property already sealed): poll instead.
    (function poll() {
      if (window.ApiClient) { patchApiClientProto(window.ApiClient); }
      else { setTimeout(poll, 50); }
    }());
  }

}());

// Intercept XHR to add Fields=ChildCount to the @jellyfin/sdk shadow call
// /Items/{uuid}?userId=... Use explicit _open.call() so the rewritten URL
// is guaranteed to reach the native implementation.
(function () {
  var SDK_RE = /^\/Items\/[0-9a-f-]{36}$/i;
  var _open = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url) {
    if (typeof url === 'string' && method.toUpperCase() === 'GET') {
      try {
        var p = new URL(url, location.href);
        if (SDK_RE.test(p.pathname) && !p.searchParams.has('Fields')) {
          p.searchParams.set('Fields', 'ChildCount');
          var rewritten = p.toString();
          var args = Array.prototype.slice.call(arguments);
          args[1] = rewritten;
          return _open.apply(this, args);
        }
      } catch (ex) {}
    }
    return _open.apply(this, arguments);
  };
}());
"#;
