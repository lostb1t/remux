use crate::{AppContext, db};

/// Resolves `media.external_ids.imdb` if it is not already set.
///
/// Uses the media's existing `external_ids` (TMDB, TVDB, etc.) to look up the
/// IMDB ID via TMDB. Returns `true` if an IMDB ID is present or was resolved,
/// `false` if it could not be determined.
pub(crate) async fn resolve_media_imdb(
    media: &mut db::Media,
    ctx: &AppContext,
) -> bool {
    if media
        .external_ids
        .imdb
        .is_some()
    {
        return true;
    }

    let is_tv = matches!(media.kind, db::MediaKind::Series);
    let Some(client) = crate::common::tmdb_client(&ctx.db).await else {
        return false;
    };

    let Some(imdb) =
        crate::addons::tmdb::resolve_imdb_from_ids(&media.external_ids, is_tv, &client)
            .await
    else {
        return false;
    };

    media
        .external_ids
        .imdb = Some(imdb);
    true
}
