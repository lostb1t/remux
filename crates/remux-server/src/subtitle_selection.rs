//! Select external subtitles for a particular video release. Keep this independent
//! of API responses so playback, subtitle downloads, and future item responses
//! use the same ordering and stream indexes.

use crate::addons::SubtitleInfo;

fn filename_stem(name: &str) -> &str {
    let basename = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name);
    basename
        .rsplit_once('.')
        .filter(|(_, ext)| {
            matches!(
                ext.to_ascii_lowercase()
                    .as_str(),
                "srt" | "vtt" | "ass" | "ssa" | "sub" | "mkv" | "mp4" | "avi"
            )
        })
        .map_or(basename, |(stem, _)| stem)
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, strum_macros::Display, strum_macros::EnumString,
)]
#[strum(serialize_all = "PascalCase", ascii_case_insensitive)]
pub(crate) enum SubtitleMarker {
    Forced,
    Sdh,
    Hi,
}

pub(crate) fn has_subtitle_marker(name: Option<&str>, marker: SubtitleMarker) -> bool {
    name.is_some_and(|name| {
        name.split(|c: char| !c.is_alphanumeric())
            .any(|token| {
                token
                    .parse::<SubtitleMarker>()
                    .is_ok_and(|found| found == marker)
            })
    })
}

/// A subtitle's hearing-impaired flag from raw addon metadata (filename/title).
/// The bare token "hi" is ambiguous with the ISO 639-1 Hindi language code, so
/// it's only honored as a hearing-impaired marker when the subtitle itself
/// isn't tagged Hindi — otherwise a plain Hindi translation named
/// `Movie.hi.srt` gets mislabeled as hearing-impaired.
pub(crate) fn has_hi_marker(name: Option<&str>, lang: Option<&str>) -> bool {
    has_subtitle_marker(name, SubtitleMarker::Sdh)
        || (normalized_language(lang) != "hi"
            && has_subtitle_marker(name, SubtitleMarker::Hi))
}

fn subtitle_hint(sub: &SubtitleInfo) -> &str {
    if let Some(filename) = sub
        .filename
        .as_deref()
    {
        return filename;
    }
    match &sub.url {
        Some(crate::stream::StreamDescriptor::Http { url, .. }) => url.as_str(),
        Some(crate::stream::StreamDescriptor::Local(path)) => path
            .to_str()
            .unwrap_or(""),
        Some(crate::stream::StreamDescriptor::Opendal { path, .. }) => path.as_str(),
        _ => "",
    }
}

pub(crate) fn normalized_language(lang: Option<&str>) -> String {
    lang.and_then(remux_sdks::remux::lang_to_two_letter)
        .unwrap_or_else(|| {
            lang.unwrap_or("und")
                .trim()
                .to_ascii_lowercase()
        })
}

fn language(sub: &SubtitleInfo) -> String {
    normalized_language(
        sub.lang
            .as_deref(),
    )
}

/// Only an actual subtitle filename matching the selected video filename
/// counts. Do not infer a release match from title words or a similar name.
pub(crate) fn is_release_match(
    sub: &SubtitleInfo,
    source_filename: Option<&str>,
) -> bool {
    sub.filename
        .as_deref()
        .zip(source_filename)
        .is_some_and(|(subtitle, video)| {
            let subtitle = filename_stem(subtitle);
            !subtitle.is_empty() && subtitle.eq_ignore_ascii_case(filename_stem(video))
        })
}

/// Ranks candidates release-aware and by language preference, without
/// deduping to one-per-language. `select_external_subtitles` reduces this to
/// its public one-per-language contract; kept separate so tests can inspect
/// full ranking order.
fn ranked_external_subtitles<'a>(
    subs: &'a [SubtitleInfo],
    preferred_languages: &[String],
    source_filename: Option<&str>,
) -> Vec<&'a SubtitleInfo> {
    let preferred: Vec<String> = preferred_languages
        .iter()
        .map(|lang| normalized_language(Some(lang)))
        .collect();
    let mut candidates: Vec<_> = subs
        .iter()
        .filter_map(|sub| {
            let lang = language(sub);
            let rank = preferred
                .iter()
                .position(|preferred| preferred == &lang);
            if !preferred.is_empty() && rank.is_none() {
                return None;
            }
            Some((
                sub,
                lang,
                rank.unwrap_or(usize::MAX),
                is_release_match(sub, source_filename),
            ))
        })
        .collect();

    candidates.sort_by(
        |(a, lang_a, rank_a, match_a), (b, lang_b, rank_b, match_b)| {
            rank_a
                .cmp(rank_b)
                .then_with(|| lang_a.cmp(lang_b))
                .then_with(|| match_b.cmp(match_a))
                .then_with(|| {
                    a.is_forced
                        .cmp(&b.is_forced)
                })
                .then_with(|| {
                    a.is_hi
                        .cmp(&b.is_hi)
                })
                .then_with(|| {
                    a.ai_translated
                        .unwrap_or(false)
                        .cmp(
                            &b.ai_translated
                                .unwrap_or(false),
                        )
                })
                .then_with(|| {
                    b.from_trusted
                        .unwrap_or(false)
                        .cmp(
                            &a.from_trusted
                                .unwrap_or(false),
                        )
                })
                .then_with(|| {
                    a.id.cmp(&b.id)
                })
                .then_with(|| subtitle_hint(a).cmp(subtitle_hint(b)))
        },
    );

    candidates
        .into_iter()
        .map(|(sub, ..)| sub)
        .collect()
}

/// Returns the top `max_per_language` release-aware subtitle choices for each
/// language (in rank order). Pass `1` for the pre-existing "one stable choice
/// per language" behavior.
pub(crate) fn select_external_subtitles<'a>(
    subs: &'a [SubtitleInfo],
    preferred_languages: &[String],
    source_filename: Option<&str>,
    max_per_language: usize,
) -> Vec<&'a SubtitleInfo> {
    let mut counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    ranked_external_subtitles(subs, preferred_languages, source_filename)
        .into_iter()
        .filter(|sub| {
            let count = counts
                .entry(language(sub))
                .or_insert(0);
            if *count < max_per_language {
                *count += 1;
                true
            } else {
                false
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(id: &str, filename: &str, language: &str) -> SubtitleInfo {
        SubtitleInfo {
            id: id.into(),
            url: None,
            lang: Some(language.into()),
            is_forced: false,
            is_hi: false,
            filename: Some(filename.into()),
            from_trusted: None,
            ai_translated: None,
        }
    }

    #[test]
    fn matching_release_discards_other_releases_and_prefers_human_trusted() {
        let mut trusted = sub("trusted", "MOVIE.2026.1080p.WEB-DL.SRT", "eng");
        trusted.from_trusted = Some(true);
        let mut ai = sub("ai", "Movie.2026.1080p.WEB-DL.srt", "eng");
        ai.ai_translated = Some(true);
        let other = sub("other", "Movie.2026.1080p.BluRay.srt", "eng");
        let subs = [other, ai, trusted];
        let selected = select_external_subtitles(
            &subs,
            &["en".into()],
            Some("Movie.2026.1080p.WEB-DL.mkv"),
            1,
        );
        assert_eq!(
            selected
                .iter()
                .map(|s| s
                    .id
                    .as_str())
                .collect::<Vec<_>>(),
            vec!["trusted"]
        );
    }

    #[test]
    fn no_release_match_keeps_one_fallback_in_stable_order() {
        let subs = [
            sub("b", "Movie.2026.WEBRip.srt", "eng"),
            sub("a", "Movie.2026.WEBRip.srt", "eng"),
            sub("c", "Movie.2026.TELESYNC.srt", "eng"),
        ];
        let selected =
            select_external_subtitles(&subs, &[], Some("Movie.2026.BluRay.mkv"), 1);
        assert_eq!(
            selected
                .iter()
                .map(|s| s
                    .id
                    .as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
    }

    #[test]
    fn regular_subtitle_wins_when_no_other_variant_matches() {
        let regular = sub("regular", "Movie.2026.1080p.WEB-DL.srt", "eng");
        let mut forced = sub("forced", "Movie.2026.1080p.WEB-DL.en.forced.srt", "eng");
        forced.is_forced = true;
        let mut sdh = sub("sdh", "Movie.2026.1080p.WEB-DL (SDH).srt", "eng");
        sdh.is_hi = true;
        let subs = [regular, forced, sdh];
        let selected = select_external_subtitles(
            &subs,
            &[],
            Some("Movie.2026.1080p.WEB-DL.mkv"),
            1,
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "regular");
    }

    #[test]
    fn max_per_language_above_one_returns_multiple_ranked_candidates() {
        let regular = sub("regular", "Movie.2026.1080p.WEB-DL.srt", "eng");
        let mut forced = sub("forced", "Movie.2026.1080p.WEB-DL.en.forced.srt", "eng");
        forced.is_forced = true;
        let dutch = sub("dutch", "Movie.2026.1080p.WEB-DL.srt", "dut");
        let subs = [regular, forced, dutch];
        let selected = select_external_subtitles(
            &subs,
            &[],
            Some("Movie.2026.1080p.WEB-DL.mkv"),
            2,
        );
        // Two English candidates (regular ranks first) plus the one Dutch —
        // Dutch never had a second candidate to add, so it's still just one.
        assert_eq!(
            selected
                .iter()
                .map(|s| s
                    .id
                    .as_str())
                .collect::<Vec<_>>(),
            vec!["regular", "forced", "dutch"]
        );
    }

    #[test]
    fn filenames_match_only_without_extension_and_case() {
        let subtitle = sub("title", "movie.2026.SRT", "eng");
        assert!(is_release_match(&subtitle, Some("Movie.2026.mkv")));
        let punctuation = sub("punctuation", "Movie.2026.WEBDL.srt", "eng");
        assert!(!is_release_match(
            &punctuation,
            Some("Movie.2026.WEB-DL.mkv")
        ));
    }

    #[test]
    fn language_suffix_is_not_an_exact_filename_match() {
        let subtitle = sub("turkish", "Movie.2026.1080p.WEB-DL.tr.srt", "tur");
        assert!(!is_release_match(
            &subtitle,
            Some("Movie.2026.1080p.WEB-DL.mkv")
        ));
    }

    #[test]
    fn stremio_release_metadata_deserializes() {
        let sub: remux_sdks::stremio::Subtitle = serde_json::from_str(
            r#"{"id":"42","url":"https://example.test/sub.vtt","lang":"eng","subtitleFileName":"Movie.2026.en.srt","from_trusted":true,"ai_translated":false}"#,
        )
        .unwrap();
        assert_eq!(
            sub.subtitle_file_name
                .as_deref(),
            Some("Movie.2026.en.srt")
        );
        assert_eq!(sub.from_trusted, Some(true));
        assert_eq!(sub.ai_translated, Some(false));
    }
}
