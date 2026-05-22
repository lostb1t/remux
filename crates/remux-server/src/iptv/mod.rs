pub mod m3u;
pub mod xmltv;
pub mod xtream;

pub use m3u::{M3uChannel, parse_m3u_stream};
pub use xmltv::{EpgProgram, parse_xmltv};
pub use xtream::{fetch_xtream_categories, fetch_xtream_channels};

use crate::db::ProgramKind;

/// Map a free-text category/group string to a `ProgramKind`.
/// Used for both XMLTV `<category>` tags and M3U/Xtream group-title values.
pub fn parse_program_kind(category: &str) -> Option<ProgramKind> {
    let lower = category.to_lowercase();
    let rules: &[(&[&str], ProgramKind)] = &[
        (
            &[
                "movie", "film", "cinema", "cine", "vod", "pelicul", "filme", "kino",
            ],
            ProgramKind::Movie,
        ),
        (
            &[
                "series",
                "episode",
                "soap",
                "sitcom",
                "show",
                "telenovela",
                "serial",
                "miniseries",
            ],
            ProgramKind::Series,
        ),
        (
            &[
                "news",
                "info",
                "actualit",
                "journalism",
                "documentary",
                "current affairs",
                "noticias",
            ],
            ProgramKind::News,
        ),
        (
            &[
                "children", "kids", "youth", "cartoon", "enfant", "jeunesse", "family",
                "disney", "infantil", "kinder",
            ],
            ProgramKind::Kids,
        ),
        (
            &[
                "sport",
                "basketball",
                "baseball",
                "football",
                "soccer",
                "tennis",
                "cricket",
                "golf",
                "rugby",
                "hockey",
                "racing",
                "boxing",
                "wrestling",
                "fighting",
                "mma",
            ],
            ProgramKind::Sports,
        ),
    ];
    rules
        .iter()
        .find(|(terms, _)| terms.iter().any(|t| lower.contains(t)))
        .map(|(_, kind)| kind.clone())
}
