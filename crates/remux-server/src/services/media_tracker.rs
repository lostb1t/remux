//! Turning a stored media row into what a media tracker needs to see.
//!
//! A queued delivery names the item rather than describing it, so this runs
//! once per delivery instead of once per request. That is what keeps the walk
//! to the series, and any future completion of the item's external ids, off
//! the playback path.

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
/// carries an id one could match on.
///
/// Episodes carry their series as well as their own ids, because which of the
/// two identifies an episode is the provider's choice: Yamtrack reads the
/// episode's ids, Trakt reads the show's plus season and episode numbers.
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
