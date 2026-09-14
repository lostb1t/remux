//! Release-date calendar: the event query behind the JSON view and the ICS feed.
//!
//! Events come straight from stored release dates. Nothing here calls a metadata
//! provider: the scanner and metadata refresh tasks already keep `released_at`
//! and `digital_released_at` current, so the calendar is a read over local data.

pub mod ics;

use crate::{AppContext, api, db};
use anyhow::Result;
use chrono::{Duration, NaiveDate, NaiveDateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

pub use ics::{CalendarEvent, serialize_ics};

/// Widest range a caller may request, in days. Bounds the query and the response
/// so a hand-written `from`/`to` cannot walk the entire library.
const MAX_RANGE_DAYS: i64 = 93;

/// Hard cap on events in one response. A feed past this size is not usable in a
/// calendar client, and serializing it wastes the request.
const MAX_EVENTS: u32 = 5_000;
/// Days of history the subscribable feed includes. Recent releases stay visible
/// when a client refreshes after a gap.
const FEED_PAST_DAYS: i64 = 31;

/// Days ahead the subscribable feed includes.
const FEED_FUTURE_DAYS: i64 = 61;

/// An inclusive date range, guaranteed non-inverted and within [`MAX_RANGE_DAYS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateRange {
    from: NaiveDate,
    to: NaiveDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DateRangeError {
    #[error("from must not be after to")]
    Inverted,
    #[error("range must not exceed {MAX_RANGE_DAYS} days")]
    TooWide,
}

impl DateRange {
    /// Parses a caller-supplied range, rejecting inverted or over-wide spans.
    pub fn new(from: NaiveDate, to: NaiveDate) -> Result<Self, DateRangeError> {
        if from > to {
            return Err(DateRangeError::Inverted);
        }
        if (to - from).num_days() + 1 > MAX_RANGE_DAYS {
            return Err(DateRangeError::TooWide);
        }
        Ok(Self { from, to })
    }

    /// The fixed window used by the subscribable feed, anchored on today (UTC).
    ///
    /// Feed clients cannot pass a range, so this is not caller input and is
    /// constructed directly rather than through the validating path.
    pub fn feed_window(today: NaiveDate) -> Self {
        Self {
            from: today - Duration::days(FEED_PAST_DAYS),
            to: today + Duration::days(FEED_FUTURE_DAYS),
        }
    }

    pub fn from(&self) -> NaiveDate {
        self.from
    }

    pub fn to(&self) -> NaiveDate {
        self.to
    }
}

/// Loads a user's release events in `range`, honouring their library policy.
///
/// Movies and episodes share the same premiere-date expression, so they are
/// fetched in one query. Besides avoiding duplicate work, this makes
/// `MAX_EVENTS` a real cap on the complete response rather than a per-kind cap.
pub async fn events_for_user(
    ctx: &AppContext,
    user: &db::User,
    range: DateRange,
) -> Result<Vec<CalendarEvent>> {
    let from = range
        .from
        .and_hms_opt(0, 0, 0)
        .expect("midnight is a valid time");
    // Include every representable timestamp on the end date, not only the
    // second exactly at 23:59:59.
    let to = range
        .to
        .and_hms_nano_opt(23, 59, 59, 999_999_999)
        .expect("end of day is a valid time");

    let items = query_kind(
        ctx,
        user,
        &[db::MediaKind::Movie, db::MediaKind::Episode],
        from,
        to,
    )
    .await?;
    let series_titles = series_titles(ctx, &items).await?;
    let mut events: Vec<CalendarEvent> = items
        .into_iter()
        .filter_map(|item| to_event(item, &series_titles))
        .collect();

    sort_events(&mut events);
    Ok(events)
}

/// Titles of the series owning `items`, keyed by series id. Empty for non-episodes.
async fn series_titles(
    ctx: &AppContext,
    items: &[db::Media],
) -> Result<HashMap<Uuid, String>> {
    let mut ids: Vec<Uuid> = items
        .iter()
        .filter(|i| i.kind == db::MediaKind::Episode)
        .filter_map(|i| i.grandparent_id)
        .collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    // A followed series contributes many episodes; dedupe before querying.
    ids.sort_unstable();
    ids.dedup();
    Ok(db::Media::get_by_ids(&ctx.db, &ids)
        .await?
        .into_iter()
        .map(|series| (series.id, series.title))
        .collect())
}

/// Fetches release-dated movies and episodes within the window.
async fn query_kind(
    ctx: &AppContext,
    user: &db::User,
    kinds: &[db::MediaKind],
    from: NaiveDateTime,
    to: NaiveDateTime,
) -> Result<Vec<db::Media>> {
    let policy = user
        .policy
        .as_ref();
    let result = db::Media::get_by_filter(
        &ctx.db,
        &db::MediaFilter {
            kind: Some(kinds.to_vec()),
            // Bound the window on the premiere date itself. Deliberately not
            // `digital_released_before`: that gate hides recent theatrical-only
            // movies, which are exactly what an upcoming-release calendar shows.
            premiere_after: Some(from),
            premiere_before: Some(to),
            // A Stremio-backed library holds far more than the user intends to
            // watch, so an unscoped calendar would be thousands of irrelevant
            // events. Restrict to followed items: any episode of a series they
            // have watched, or a movie/series they favorited or liked.
            tracked_by_user: Some(user.id),
            limit: Some(MAX_EVENTS),
            user_id: Some(user.id),
            sort_by: vec![api::ItemSortBy::PremiereDate],
            sort_order: vec![api::SortOrder::Ascending],
            max_parental_rating: policy.and_then(|p| p.max_parental_rating),
            blocked_tags: policy
                .map(|p| {
                    p.blocked_tags
                        .clone()
                })
                .filter(|v| !v.is_empty()),
            allowed_tags: policy
                .map(|p| {
                    p.allowed_tags
                        .clone()
                })
                .filter(|v| !v.is_empty()),
            policy_filter: policy
                .and_then(|p| {
                    p.filter_rules
                        .as_ref()
                })
                .cloned(),
            ..Default::default()
        },
    )
    .await?;
    Ok(result.records)
}

/// Converts a media row into an event, or `None` when it has no usable date.
fn to_event(
    item: db::Media,
    series_titles: &HashMap<Uuid, String>,
) -> Option<CalendarEvent> {
    let date = premiere_date(&item)?;

    let (series_title, season_number, episode_number) =
        if item.kind == db::MediaKind::Episode {
            let title = item
                .grandparent_id
                .and_then(|id| series_titles.get(&id))
                .cloned();
            (title, item.parent_idx, item.idx)
        } else {
            (None, None, None)
        };

    Some(CalendarEvent {
        id: item
            .id
            .to_string(),
        date,
        title: item.title,
        series_title,
        season_number,
        episode_number,
        updated_at: item.updated_at,
    })
}

/// The date the item premieres: its theatrical or air date, else its digital
/// release. This is `COALESCE(released_at, digital_released_at)` — deliberately
/// the same expression `premiere_after`/`premiere_before` bound and
/// `ItemSortBy::PremiereDate` sorts on, so every returned item renders on a date
/// inside the requested window.
fn premiere_date(item: &db::Media) -> Option<NaiveDate> {
    item.released_at
        .or(item.digital_released_at)
        .map(|d| d.date())
}

/// Orders events by date, then series, season, and episode, so a series' episodes
/// read in broadcast order when several land on the same day.
fn sort_events(events: &mut [CalendarEvent]) {
    events.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then_with(|| {
                a.series_title
                    .cmp(&b.series_title)
            })
            .then_with(|| {
                a.season_number
                    .cmp(&b.season_number)
            })
            .then_with(|| {
                a.episode_number
                    .cmp(&b.episode_number)
            })
            .then_with(|| {
                a.title
                    .cmp(&b.title)
            })
            .then_with(|| {
                a.id.cmp(&b.id)
            })
    });
}

/// Today's date in UTC. All-day events carry no time, so the server's local
/// offset would only shift the window's edges by a day.
pub fn today() -> NaiveDate {
    Utc::now().date_naive()
}
