//! Turning a stored media row into what a tracking provider needs to see.
//!
//! This runs once per delivery rather than once per request, which is what
//! keeps the walk to the series off the playback path.

use anyhow::Result;
use chrono::Datelike;
use sqlx::SqlitePool;

use crate::{addons::tracking::TrackingTarget, db};

fn describe(media: &db::Media, series: Option<&db::Media>) -> TrackingTarget {
    TrackingTarget {
        kind: media
            .kind
            .clone(),
        title: media
            .title
            .clone(),
        year: media
            .released_at
            .map(|d| d.year()),
        ids: media
            .external_ids
            .clone(),
        series: series.map(|s| Box::new(describe(s, None))),
        season: media.parent_idx,
        episode: media.idx,
    }
}

/// The item as a provider needs to see it, or `None` when nothing about it
/// carries an id one could match on. Episodes carry their series, because a
/// provider keys an episode on the show's ids plus season and episode.
pub async fn resolve_target(
    db: &SqlitePool,
    media: &db::Media,
) -> Result<Option<TrackingTarget>> {
    let series = if media.kind == db::MediaKind::Episode {
        db::Media::get_ancestors(db, &media.id)
            .await?
            .into_iter()
            .find(|m| m.kind == db::MediaKind::Series)
    } else {
        None
    };

    let target = describe(media, series.as_ref());
    Ok(target
        .is_matchable()
        .then_some(target))
}
