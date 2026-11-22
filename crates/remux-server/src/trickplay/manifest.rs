//! Jellyfin's `Trickplay` field, and the sheet names the route accepts.

use super::*;

/// Jellyfin's `Trickplay` entry for one length in ticks, or `None` when that
/// length is unknown, under 5 minutes or over 4 hours.
pub(super) fn manifest_entry(
    ticks: Option<i64>,
) -> Option<HashMap<String, api::TrickplayInfoDto>> {
    let ticks = ticks.filter(|t| *t > 0)?;
    length_ok(u64::try_from(ticks / TICKS_PER_SEC).ok()).ok()?;
    let info = api::TrickplayInfoDto {
        width: WIDTH as i32,
        height: HEIGHT as i32,
        tile_width: TILE_WIDTH as i32,
        tile_height: TILE_HEIGHT as i32,
        thumbnail_count: i32::try_from(picture_count(ticks)).unwrap_or(i32::MAX),
        interval: (INTERVAL_SECS * 1000) as i32,
        bandwidth: BANDWIDTH,
    };
    Some(HashMap::from([(WIDTH.to_string(), info)]))
}

/// Jellyfin's `Trickplay` field for an item: one entry per media source whose
/// row passes `row_ok`, keyed by the source id in simple form, then by width;
/// with no media sources listed, one entry under the item id. Each source is
/// judged and counted by its own length, the item's when it lists none, so a
/// file holding two episodes or a cut of another length is told its own
/// count. Empty for anything but a film, episode or stream, and for a source
/// under 5 minutes or over 4 hours long.
pub fn manifest(
    dto: &api::BaseItemDto,
    row_ok: impl Fn(&str) -> bool,
) -> HashMap<String, HashMap<String, api::TrickplayInfoDto>> {
    let mut out = HashMap::new();
    let playable = matches!(
        dto.type_,
        api::MediaType::Movie | api::MediaType::Episode | api::MediaType::Video
    );
    if !playable || dto.is_live == Some(true) {
        return out;
    }
    let item_ticks = dto
        .run_time_ticks
        .filter(|t| *t > 0);
    for source in dto
        .media_sources
        .iter()
        .flatten()
    {
        if source.is_infinite_stream {
            continue;
        }
        let ticks = source
            .run_time_ticks
            .filter(|t| *t > 0)
            .or(item_ticks);
        let Some(entry) = manifest_entry(ticks) else {
            continue;
        };
        let id = source
            .id
            .as_simple()
            .to_string();
        if !row_ok(&id) {
            continue;
        }
        out.insert(id, entry);
    }
    // A list asked for without `MediaSources`: the first source a player is
    // given carries the item id, so that is the key it looks up.
    if dto
        .media_sources
        .is_none()
        && let Some(entry) = manifest_entry(item_ticks)
    {
        let id = dto
            .id
            .as_simple()
            .to_string();
        if row_ok(&id) {
            out.insert(id, entry);
        }
    }
    out
}

/// The sheet number in a requested file name: ASCII digits with no sign and
/// no leading zero (except `0` itself), at most 6 of them, then `.jpg`.
pub fn parse_sheet(file: &str) -> Option<u32> {
    let digits = file.strip_suffix(".jpg")?;
    let valid = !digits.is_empty()
        && digits.len() <= 6
        && digits
            .bytes()
            .all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'));
    if !valid {
        return None;
    }
    digits
        .parse()
        .ok()
}
