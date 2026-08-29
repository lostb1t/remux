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

  /* ── ElegantFin mobile: pull logo up so it clears the play button ── */
  .layout-mobile .detailLogo { top: 30vh !important; }

  /* ── Card listing: hide inline play button overlay ───────────── */
  .cardOverlayButton-br { display: none !important; }

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

  /* Keep the header's actual controls available to the pointer even when an
     older webOS engine leaves a non-interactive state on the header shell. */
  html.layout-tv .skinHeader .headerTop,
  html.layout-tv .skinHeader .headerTabs,
  html.layout-tv .skinHeader button,
  html.layout-tv .skinHeader a,
  html.layout-tv .skinHeader [role="button"] {
    pointer-events: auto !important;
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
    // Jellyfin caches views: multiple detail-page trees may exist in the DOM
    // (one per cached view). Anchor on the always-visible primary container and
    // use offsetParent to find the visible view.
    var all = document.querySelectorAll('.detailPagePrimaryContainer');
    for (var i = 0; i < all.length; i++) {
      if (all[i].offsetParent !== null) return all[i].querySelector('.detailPagePrimaryContent');
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

  function hideTrackControls(page) {
    var form = page.querySelector('.trackSelections');
    if (!form) return;
    var containers = form.querySelectorAll('.selectSourceContainer, .selectVideoContainer, .selectAudioContainer, .selectSubtitlesContainer');
    for (var i = 0; i < containers.length; i++) containers[i].classList.add('hide');
  }

  function showSpinner(page) {
    removeSpinner(page);
    var form = page.querySelector('.trackSelections');
    if (!form) return;
    // Hide the stub-rendered selects but keep the outer panel: the spinner
    // renders inside it, centered like the track fields do after load.
    hideTrackControls(page);
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
    hideTrackControls(page);
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
    // Same guard as renderAsyncTrackSelections: the version-change handler
    // re-renders the track selects, and the observer must not loop on that.
    var form = page.querySelector('.trackSelections');
    if (form) form._remuxRendering = true;
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
    if (form) setTimeout(function () { form._remuxRendering = false; }, 0);
  }

  // The core re-renders the track selects from its cached item (the fast item
  // with stub MediaSources) on player changes and cached-view restores, wiping
  // our real audio/subtitle dropdowns. Re-apply our loaded data whenever the
  // core touches the panel.
  function attachTrackSelectionsGuard(page) {
    var form = page.querySelector('.trackSelections');
    if (!form || form._remuxObsAttached) return;
    form._remuxObsAttached = true;
    var obs = new MutationObserver(function () {
      if (form._remuxRendering) return;
      if (!form._remuxLoaded) return;
      var ms = window._remuxCurrentMediaSources;
      if (!ms || !ms.length) return;
      renderAsyncTrackSelections(page, ms);
    });
    obs.observe(form, { childList: true, subtree: true });
  }

  function renderAsyncTrackSelections(page, mediaSources) {
    var form = page.querySelector('.trackSelections');
    if (!form) return;
    // Guard: the MutationObserver below must not react to our own renders.
    // Observer callbacks run as microtasks before the next macrotask, so a
    // setTimeout(0) clear happens after any observer callback queued by this
    // render has already run (and been skipped).
    form._remuxRendering = true;

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

    setTimeout(function () { form._remuxRendering = false; }, 0);
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

  function patchApiClientProto(apiClient) {
    var proto = Object.getPrototypeOf(apiClient);
    if (!proto || proto._remuxGetItemPatched) return;
    proto._remuxGetItemPatched = true;

    // Only getItem owns the follow-up MediaSources request below. Generic item
    // fetches must remain intact: reducing one to ChildCount would leave callers
    // that bypass getItem with a permanent stub and no source picker.
    proto.getItem = function (userId, itemId) {
      var self = this;
      // Guard: itemId may be non-string (e.g. undefined) when called from list-view play buttons.
      if (typeof itemId !== 'string') {
        return self.getJSON(self.getUrl('Users/' + userId + '/Items/' + itemId));
      }
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
                attachTrackSelectionsGuard(page);
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

// Jellyfin deliberately disables its global focus-follows-pointer behavior on
// webOS. Keep the Magic Remote pointer and keyboard focus aligned only for the
// shared header. Focusing a TV button scales it; some webOS versions then lose
// the native click between press and release because the target geometry moved.
// Capture that gesture and dispatch exactly one click to the pressed control.
(function () {
  if (!/(web0s|netcast)/i.test(navigator.userAgent || '')) return;

  var pressedHeaderControl = null;
  var pressedAt = null;
  var dispatchingHeaderClick = false;
  var lastActivatedHeaderControl = null;
  var suppressClickUntil = 0;
  var lastPointer = null;

  function hasOpenDialog() {
    var dialogs = document.querySelectorAll('.dialog');
    for (var i = 0; i < dialogs.length; i++) {
      if (!dialogs[i].classList.contains('hide')) return true;
    }
    return false;
  }

  function headerControlFrom(target) {
    var header = document.querySelector('.skinHeader');
    if (!header || !target || !header.contains(target)) return null;

    while (target && target !== header) {
      var tagName = target.tagName;
      if ((tagName === 'BUTTON'
          || tagName === 'A'
          || target.getAttribute('role') === 'button')
          && !target.disabled) {
        return target;
      }
      target = target.parentElement;
    }
    return null;
  }

  function eventPoint(event) {
    if (typeof event.clientX !== 'number' || typeof event.clientY !== 'number') return null;
    return { x: event.clientX, y: event.clientY, at: Date.now() };
  }

  function headerControlAtPoint(point) {
    var header = document.querySelector('.skinHeader');
    if (!header || !point) return null;
    var controls = header.querySelectorAll('button, a, [role="button"]');
    for (var i = 0; i < controls.length; i++) {
      var control = controls[i];
      if (control.disabled) continue;
      var rect = control.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0
          && point.x >= rect.left && point.x <= rect.right
          && point.y >= rect.top && point.y <= rect.bottom) {
        return control;
      }
    }
    return null;
  }

  function headerControlForEvent(event) {
    // Modal dialogs (including audio/subtitle action sheets) own the gesture.
    // Never route their pointer events through to a header control behind them.
    if (hasOpenDialog()) return null;
    var direct = headerControlFrom(event.target);
    if (direct) return direct;
    var point = eventPoint(event);
    return headerControlAtPoint(point || lastPointer);
  }

  function focusControl(control) {
    if (!control || document.activeElement === control) return;
    try {
      control.focus({ preventScroll: true });
    } catch (error) {
      control.focus();
    }
  }

  function focusHeaderControl(event) {
    var point = eventPoint(event);
    if (point) lastPointer = point;
    focusControl(headerControlForEvent(event));
  }

  function activateHeaderControl(control, event) {
    if (!control || !document.documentElement.contains(control)) return;
    if (event) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
    lastActivatedHeaderControl = control;
    suppressClickUntil = Date.now() + 500;
    dispatchingHeaderClick = true;
    try {
      control.click();
    } finally {
      dispatchingHeaderClick = false;
    }
  }

  function pressHeaderControl(event) {
    var point = eventPoint(event);
    if (point) lastPointer = point;
    var control = headerControlForEvent(event);
    if (!control || (typeof event.button === 'number' && event.button !== 0)) {
      pressedHeaderControl = null;
      pressedAt = null;
      return;
    }

    pressedHeaderControl = control;
    pressedAt = point;
    focusControl(control);
    event.preventDefault();
    event.stopImmediatePropagation();
  }

  function releaseHeaderControl(event) {
    var control = pressedHeaderControl;
    var start = pressedAt;
    pressedHeaderControl = null;
    pressedAt = null;
    if (hasOpenDialog()) return;
    if (!control || !document.documentElement.contains(control)) return;
    if (start && typeof event.clientX === 'number'
        && (Math.abs(event.clientX - start.x) > 24
            || Math.abs(event.clientY - start.y) > 24)) return;

    activateHeaderControl(control, event);
  }

  function routeHeaderClick(event) {
    if (dispatchingHeaderClick) return;
    if (hasOpenDialog()) return;
    var directControl = headerControlFrom(event.target);
    var control = directControl || headerControlForEvent(event);
    if (control === lastActivatedHeaderControl && Date.now() <= suppressClickUntil) {
      lastActivatedHeaderControl = null;
      event.preventDefault();
      event.stopImmediatePropagation();
      return;
    }
    // A webOS overlay can own the click target even when the pointer is visibly
    // over a header button. Resolve that button from the event coordinates.
    if (!directControl) activateHeaderControl(control, event);
  }

  function selectPointedHeaderControl(event) {
    var key = event.key || event.keyCode;
    if (key === 'ArrowLeft' || key === 'ArrowRight'
        || key === 'ArrowUp' || key === 'ArrowDown'
        || key === 37 || key === 38 || key === 39 || key === 40) {
      lastPointer = null;
      return;
    }
    if (key !== 'Enter' && key !== 13) return;

    var control = headerControlFrom(document.activeElement);
    if (!control && lastPointer && Date.now() - lastPointer.at <= 6000) {
      control = headerControlAtPoint(lastPointer);
    }
    if (!control) return;
    focusControl(control);
    activateHeaderControl(control, event);
  }

  document.addEventListener('pointerover', focusHeaderControl, true);
  document.addEventListener('pointermove', focusHeaderControl, true);
  document.addEventListener('pointerdown', pressHeaderControl, true);
  document.addEventListener('pointerup', releaseHeaderControl, true);
  document.addEventListener('pointercancel', function () {
    pressedHeaderControl = null;
    pressedAt = null;
  }, true);
  document.addEventListener('mouseover', focusHeaderControl, true);
  document.addEventListener('mousemove', focusHeaderControl, true);
  document.addEventListener('mousedown', pressHeaderControl, true);
  document.addEventListener('mouseup', releaseHeaderControl, true);
  document.addEventListener('click', routeHeaderClick, true);
  document.addEventListener('keydown', selectPointedHeaderControl, true);
}());

// Strip "Recently Added in " prefix from homescreen section titles, leaving only the library name.
(function () {
  var PREFIX = 'Recently Added in ';

  function clean(el) {
    var walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, null, false);
    var node;
    while ((node = walker.nextNode())) {
      if (node.nodeValue && node.nodeValue.indexOf(PREFIX) === 0) {
        node.nodeValue = node.nodeValue.slice(PREFIX.length);
      }
    }
  }

  function processRoot(root) {
    if (!root.querySelectorAll) return;
    var els = root.querySelectorAll('.sectionTitle, .sectionTitleLink');
    for (var i = 0; i < els.length; i++) clean(els[i]);
    if (root.classList && (root.classList.contains('sectionTitle') || root.classList.contains('sectionTitleLink'))) {
      clean(root);
    }
  }

  new MutationObserver(function (mutations) {
    for (var i = 0; i < mutations.length; i++) {
      var added = mutations[i].addedNodes;
      for (var j = 0; j < added.length; j++) {
        if (added[j].nodeType === 1) processRoot(added[j]);
      }
    }
  }).observe(document.body, { childList: true, subtree: true });

  processRoot(document.body);
}());

"#;

#[cfg(test)]
mod tests {
    use super::{CSS, JS};

    #[test]
    fn generic_item_transports_are_not_rewritten_to_source_stubs() {
        assert!(!JS.contains("proto.fetch = function"));
        assert!(!JS.contains("XMLHttpRequest.prototype.open = function"));
    }

    #[test]
    fn webos_header_pointer_supports_pointer_mouse_and_select_events() {
        assert!(CSS.contains("html.layout-tv .skinHeader .headerTop"));
        assert!(CSS.contains("pointer-events: auto !important"));
        assert!(JS.contains("pressedHeaderControl = control;"));
        assert!(JS.contains("headerControlAtPoint"));
        assert!(JS.contains("addEventListener('pointerdown'"));
        assert!(JS.contains("selectPointedHeaderControl"));
        assert!(JS.contains("if (hasOpenDialog()) return null;"));
        assert!(
            JS.contains("var control = directControl || headerControlForEvent(event);")
        );
        assert!(JS.contains("control.click();"));
        assert!(JS.contains("routeHeaderClick"));
    }
}
