pub mod m3u;
pub mod xmltv;
pub mod xtream;

pub use m3u::{M3uChannel, parse_m3u};
pub use xmltv::{EpgProgram, parse_xmltv};
pub use xtream::{fetch_xtream_categories, fetch_xtream_channels};

use crate::db::ProgramKind;

/// Map a free-text category/group string to a `ProgramKind`.
/// Used for both XMLTV `<category>` tags and M3U/Xtream group-title values.
pub fn parse_program_kind(category: &str) -> Option<ProgramKind> {
    let lower = category.to_lowercase();
    if lower.contains("movie")
        || lower.contains("film")
        || lower.contains("cinema")
        || lower.contains("cine")
    {
        Some(ProgramKind::Movie)
    } else if lower.contains("series")
        || lower.contains("episode")
        || lower.contains("soap")
        || lower.contains("sitcom")
    {
        Some(ProgramKind::Series)
    } else if lower.contains("news")
        || lower.contains("info")
        || lower.contains("actualit")
    {
        Some(ProgramKind::News)
    } else if lower.contains("children")
        || lower.contains("kids")
        || lower.contains("youth")
        || lower.contains("cartoon")
        || lower.contains("enfant")
        || lower.contains("jeunesse")
    {
        Some(ProgramKind::Kids)
    } else if lower.contains("sport") {
        Some(ProgramKind::Sports)
    } else {
        None
    }
}
