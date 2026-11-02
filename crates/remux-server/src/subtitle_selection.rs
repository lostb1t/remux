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

/// A subtitle file sitting on the server's own disk, next to the library
/// file (a local addon's sidecar). Someone put it there for this video, so it
/// outranks anything a provider offers; no release-name guessing is needed.
///
/// The local addon returns every sidecar indexed for the episode or movie, so
/// a sidecar only gets the boost when it belongs to the video being played:
/// the video's name followed by language/flag suffixes only
/// (`Movie.WEB-DL.mkv` owns `Movie.WEB-DL.en.forced.srt`, not
/// `Movie.BluRay.en.srt`). With no source filename there is nothing to tell
/// them apart, so every sidecar counts.
fn is_local_sidecar(sub: &SubtitleInfo, source_filename: Option<&str>) -> bool {
    if !matches!(sub.url, Some(crate::stream::StreamDescriptor::Local(_))) {
        return false;
    }
    let Some(video) = source_filename else {
        return true;
    };
    let subtitle = filename_stem(subtitle_hint(sub));
    let video = filename_stem(video);
    if video.is_empty()
        || subtitle.len() < video.len()
        || !subtitle.is_char_boundary(video.len())
    {
        return false;
    }
    let (head, tail) = subtitle.split_at(video.len());
    head.eq_ignore_ascii_case(video)
        && tail
            .strip_prefix('.')
            .map_or(tail.is_empty(), |suffix| {
                suffix
                    .split('.')
                    .all(is_sidecar_suffix_token)
            })
}

/// A language code (`en`, `eng`, `pt-BR`) or a subtitle flag.
fn is_sidecar_suffix_token(token: &str) -> bool {
    matches!(
        token
            .to_ascii_lowercase()
            .as_str(),
        "forced" | "hi" | "sdh" | "cc" | "default"
    ) || (!token.is_empty()
        && token
            .split(['-', '_'])
            .all(|part| {
                (2..=4).contains(&part.len())
                    && part
                        .chars()
                        .all(|c| c.is_ascii_alphabetic())
            }))
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
                is_local_sidecar(sub, source_filename),
                is_release_match(sub, source_filename),
            ))
        })
        .collect();

    candidates.sort_by(
        |(a, lang_a, rank_a, local_a, match_a),
         (b, lang_b, rank_b, local_b, match_b)| {
            rank_a
                .cmp(rank_b)
                .then_with(|| lang_a.cmp(lang_b))
                .then_with(|| local_b.cmp(local_a))
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
    fn local_sidecar_outranks_provider_subtitles_even_with_a_release_match() {
        let mut local = sub("local", "Movie.2026.1080p.WEB-DL.en.srt", "eng");
        local.url = Some(crate::stream::StreamDescriptor::Local(
            "/media/Movie.2026.1080p.WEB-DL.en.srt".into(),
        ));
        let mut exact = sub("exact", "Movie.2026.1080p.WEB-DL.srt", "eng");
        exact.from_trusted = Some(true);
        let mut other_lang = sub("other_lang", "Movie.2026.1080p.WEB-DL.srt", "fra");
        other_lang.from_trusted = Some(true);
        let subs = [exact, other_lang, local];
        let ranked =
            ranked_external_subtitles(&subs, &[], Some("Movie.2026.1080p.WEB-DL.mkv"));
        let ids: Vec<_> = ranked
            .iter()
            .map(|s| {
                s.id.as_str()
            })
            .collect();
        // Within a language the sidecar goes first; languages stay ordered.
        assert_eq!(ids, vec!["local", "exact", "other_lang"]);
    }

    #[test]
    fn a_sidecar_of_another_release_does_not_outrank_an_exact_provider_match() {
        let mut bluray = sub("bluray", "Movie.2026.1080p.BluRay.en.srt", "eng");
        bluray.url = Some(crate::stream::StreamDescriptor::Local(
            "/media/Movie.2026.1080p.BluRay.en.srt".into(),
        ));
        let mut exact = sub("exact", "Movie.2026.1080p.WEB-DL.srt", "eng");
        exact.from_trusted = Some(true);
        let subs = [bluray, exact];
        let chosen = select_external_subtitles(
            &subs,
            &[],
            Some("Movie.2026.1080p.WEB-DL.mkv"),
            1,
        );
        assert_eq!(chosen[0].id, "exact");
    }

    #[test]
    fn sidecar_ownership_allows_only_language_and_flag_suffixes() {
        let local = |name: &str| {
            let mut s = sub("local", name, "eng");
            s.url = Some(crate::stream::StreamDescriptor::Local(
                format!("/media/{name}").into(),
            ));
            s
        };
        let video = Some("Show.S01E01.WEB-DL.mkv");
        for owned in [
            "Show.S01E01.WEB-DL.srt",
            "Show.S01E01.WEB-DL.en.srt",
            "Show.S01E01.WEB-DL.eng.forced.srt",
            "Show.S01E01.WEB-DL.pt-BR.sdh.srt",
            "show.s01e01.web-dl.EN.srt",
        ] {
            assert!(is_local_sidecar(&local(owned), video), "{owned}");
        }
        for foreign in [
            "Show.S01E01.BluRay.en.srt",
            "Show.S01E01.WEB-DL.REPACK.en.srt",
            "Show.S01E01.WEB-DL-extra.en.srt",
            "Show.S01E02.WEB-DL.en.srt",
        ] {
            assert!(!is_local_sidecar(&local(foreign), video), "{foreign}");
        }
        // No source filename: nothing to compare, so every sidecar counts.
        assert!(is_local_sidecar(&local("Anything.en.srt"), None));
        // Providers never get the sidecar boost.
        assert!(!is_local_sidecar(
            &sub("p", "Show.S01E01.WEB-DL.en.srt", "eng"),
            video
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
