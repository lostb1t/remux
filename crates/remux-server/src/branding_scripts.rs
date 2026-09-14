//! Scripts bundled with the server that an operator can switch on from the
//! branding page.
//!
//! These are deliberately *not* part of [`crate::web_patches`]: that static is
//! injected unconditionally and is reserved for the minimum needed to make the
//! stock web client work against Remux. Anything optional — a behaviour some
//! deployments want and others don't — belongs here, behind a toggle, so an
//! operator opts in.
//!
//! They are also kept out of `custom_js`. Shipping a default value there would
//! overwrite whatever the operator wrote by hand, and any scheme that merges
//! into that field needs sentinel comments to find its own block again. A
//! separate id list avoids the collision entirely.

use remux_sdks::remux::BrandingScriptInfo;

pub struct BrandingScript {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub source: &'static str,
}

pub static SCRIPTS: &[BrandingScript] = &[BrandingScript {
    id: "hide_from_my_media",
    name: "Honor \"Show in My Media\"",
    description: "Hides a collection's tile from the home screen's \"My Media\" \
                  section when its \"Show in My Media\" setting is off. The \
                  collection stays in the sidebar, keeps its own home row, and \
                  remains browsable. Requires a client based on Jellyfin Web.",
    source: HIDE_FROM_MY_MEDIA,
}];

pub fn catalog() -> Vec<BrandingScriptInfo> {
    SCRIPTS
        .iter()
        .map(|s| BrandingScriptInfo {
            id: s
                .id
                .to_string(),
            name: s
                .name
                .to_string(),
            description: s
                .description
                .to_string(),
        })
        .collect()
}

/// Concatenated sources for the given ids, in registry order so the result is
/// stable regardless of the order the operator enabled them. Unknown ids are
/// skipped: a config written by a newer build must not break this one.
pub fn sources_for(enabled: &[String]) -> String {
    SCRIPTS
        .iter()
        .filter(|s| {
            enabled
                .iter()
                .any(|id| id == s.id)
        })
        .map(|s| s.source)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Hides tiles for views flagged `Remux.ShowInMyMedia === false`.
///
/// This cannot be done server-side. The home tiles, the sidebar and the
/// "Latest" shelf selection all consume one shared `GET /Users/{id}/Views`
/// response (react-query key `["User",id,"Views"]`), so dropping a view from
/// that payload removes it from all three at once — which is exactly what the
/// existing `MyMediaExcludes` list does. The rendered DOM is the only place
/// the three surfaces are distinguishable.
const HIDE_FROM_MY_MEDIA: &str = r##"
(function () {
  var hidden = null;      // { id: true } for views to skip; null until loaded
  var pending = null;     // In-flight fetch, so concurrent callers share it

  // `/userviews` already applies this user's policy and MyMediaExcludes, so a
  // view missing here is hidden for other reasons and needs no tile handling.
  function loadHidden() {
    if (hidden) return Promise.resolve(hidden);
    if (pending) return pending;
    var client = window.ApiClient;
    if (!client || !client.getUrl || !client.getJSON) return Promise.resolve(null);
    pending = client
      .getJSON(client.getUrl('UserViews', { userId: client.getCurrentUserId() }))
      .then(function (result) {
        var ids = Object.create(null);
        var items = (result && result.Items) || [];
        for (var i = 0; i < items.length; i++) {
          var item = items[i];
          var remux = item && (item.Remux || item.remux);
          if (remux && remux.ShowInMyMedia === false) ids[String(item.Id)] = true;
        }
        hidden = ids;
        pending = null;
        return hidden;
      })
      .catch(function () {
        pending = null;
        return null;
      });
    return pending;
  }

  // Both My Media flavours are identified by what the renderer emits:
  //   librarybuttons     -> <a class="raised homeLibraryButton emby-button"
  //                            href="#/list?parentId=<id>&serverId=...">
  //   smalllibrarytiles  -> <div class="card ..." data-id="<id>">
  function libraryIdFor(el) {
    if (el.classList.contains('homeLibraryButton')) {
      var href = el.getAttribute('href') || '';
      var qmark = href.indexOf('?');
      if (qmark < 0) return null;
      var params = new URLSearchParams(href.slice(qmark + 1));
      return params.get('parentId') || params.get('topParentId') || params.get('id');
    }
    // Content cards carry data-context ("home"); library tiles never do. This
    // is what keeps a Latest row — same .card/.itemsContainer shape — intact.
    if (el.hasAttribute('data-context')) return null;
    return el.getAttribute('data-id');
  }

  // A My Media section owns its heading directly (`> h2.sectionTitle`), whereas
  // a Latest row wraps it in a `.sectionTitleContainer`. That structural
  // difference is the reliable discriminator, and scoping to
  // `.homeSectionsContainer` keeps the sidebar untouched.
  function myMediaTiles() {
    var out = [];
    var sections = document.querySelectorAll('.homeSectionsContainer .verticalSection');
    for (var i = 0; i < sections.length; i++) {
      var section = sections[i];
      if (!section.querySelector(':scope > h2.sectionTitle')) continue;
      var tiles = section.querySelectorAll('.card[data-id], a.homeLibraryButton');
      for (var j = 0; j < tiles.length; j++) out.push(tiles[j]);
    }
    return out;
  }

  function apply() {
    var tiles = myMediaTiles();
    if (!tiles.length) return;
    loadHidden().then(function (ids) {
      if (!ids) return;
      for (var i = 0; i < tiles.length; i++) {
        var tile = tiles[i];
        if (!tile.isConnected) continue;
        var id = libraryIdFor(tile);
        if (id && ids[id]) tile.style.display = 'none';
      }
    });
  }

  // Home sections are rebuilt on settings change and on re-entering the tab, so
  // a one-shot pass would be undone. Coalesce with setTimeout rather than
  // requestAnimationFrame, which is throttled to never firing on hidden pages
  // and in some embedded webviews.
  var queued = false;
  new MutationObserver(function () {
    if (queued) return;
    queued = true;
    setTimeout(function () {
      queued = false;
      apply();
    }, 0);
  }).observe(document.body, { childList: true, subtree: true });

  apply();
}());
"##;
