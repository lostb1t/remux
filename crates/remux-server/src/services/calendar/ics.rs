//! RFC 5545 (iCalendar) serialization for release-date calendar feeds.
//!
//! Events are all-day `VALUE=DATE` entries, so they render on the release date
//! itself in every subscriber's timezone rather than shifting with the server's
//! offset. `UID` and `DTSTAMP` are derived from stored item data only, never
//! from the request or the feed token, so an unchanged library re-serializes
//! byte-identically and calendar clients see no spurious updates.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};

const PRODUCT_ID: &str = "-//Remux//Calendar//EN";

/// Maximum octets in one unfolded content line, per RFC 5545 §3.1.
const LINE_OCTET_LIMIT: usize = 75;

/// A single all-day release event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEvent {
    /// Stable per-item identifier; becomes the `UID`.
    pub id: String,
    /// Release date. All-day events carry no time component.
    pub date: NaiveDate,
    /// Item title. For episodes this is the episode title.
    pub title: String,
    /// Series title, set for episodes only.
    pub series_title: Option<String>,
    pub season_number: Option<i64>,
    pub episode_number: Option<i64>,
    /// Item `updated_at`; becomes `DTSTAMP`/`LAST-MODIFIED`.
    pub updated_at: NaiveDateTime,
}

impl CalendarEvent {
    /// `SUMMARY` text: `Series - S02E03 - Episode` for episodes, the bare title
    /// otherwise. Missing components are dropped rather than rendered blank.
    fn summary(&self) -> String {
        let code = match (self.season_number, self.episode_number) {
            (Some(season), Some(episode)) => Some(format!("S{season:02}E{episode:02}")),
            _ => None,
        };
        let mut parts = Vec::with_capacity(3);
        if let Some(series) = self
            .series_title
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            parts.push(series);
        }
        if let Some(code) = code.as_deref() {
            parts.push(code);
        }
        if !self
            .title
            .is_empty()
        {
            parts.push(&self.title);
        }
        parts.join(" - ")
    }
}

/// Serializes events into a `text/calendar` body with CRLF line endings.
pub fn serialize_ics(events: &[CalendarEvent]) -> String {
    // Every event contributes 7 lines that are mostly short; this keeps the
    // common feed under one allocation.
    let mut out = String::with_capacity(128 + events.len() * 220);
    write_line(&mut out, "BEGIN:VCALENDAR");
    write_line(&mut out, "VERSION:2.0");
    write_line(&mut out, &format!("PRODID:{PRODUCT_ID}"));
    write_line(&mut out, "CALSCALE:GREGORIAN");
    for event in events {
        let stamp = DateTime::<Utc>::from_naive_utc_and_offset(event.updated_at, Utc)
            .format("%Y%m%dT%H%M%SZ");
        write_line(&mut out, "BEGIN:VEVENT");
        write_line(
            &mut out,
            &format!("UID:{}", escape_text(&format!("{}@remux", event.id))),
        );
        write_line(&mut out, &format!("DTSTAMP:{stamp}"));
        write_line(&mut out, &format!("LAST-MODIFIED:{stamp}"));
        write_line(
            &mut out,
            &format!(
                "DTSTART;VALUE=DATE:{}",
                event
                    .date
                    .format("%Y%m%d")
            ),
        );
        // DTEND is exclusive: a one-day event ends on the following date.
        write_line(
            &mut out,
            &format!(
                "DTEND;VALUE=DATE:{}",
                (event.date + Duration::days(1)).format("%Y%m%d")
            ),
        );
        write_line(
            &mut out,
            &format!("SUMMARY:{}", escape_text(&event.summary())),
        );
        write_line(&mut out, "END:VEVENT");
    }
    write_line(&mut out, "END:VCALENDAR");
    out
}

/// Escapes a `TEXT` value per RFC 5545 §3.3.11.
///
/// Control characters other than TAB are stripped first: they are forbidden in
/// `TEXT` and have no escape sequence, so any that survived metadata ingestion
/// would produce an unparseable feed. Backslash is escaped before the other
/// sequences so the escapes themselves are not double-escaped.
fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value
        .chars()
        .peekable();
    while let Some(character) = chars.next() {
        match character {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            // CRLF collapses to a single escaped newline.
            '\r' => {
                if chars
                    .peek()
                    .is_some_and(|next| *next == '\n')
                {
                    chars.next();
                }
                out.push_str("\\n");
            }
            '\n' => out.push_str("\\n"),
            '\t' => out.push('\t'),
            control if control.is_control() => {}
            other => out.push(other),
        }
    }
    out
}

/// Appends one content line, folded at 75 octets per RFC 5545 §3.1.
///
/// Folding counts octets, not characters, and a multi-byte character must never
/// be split across the fold, so the break is taken at the last character
/// boundary that fits. Continuation lines begin with a single space, which
/// counts against the limit.
fn write_line(out: &mut String, line: &str) {
    let mut rest = line;
    let mut first = true;
    while !rest.is_empty() {
        let mut limit = LINE_OCTET_LIMIT;
        if !first {
            out.push(' ');
            limit -= 1;
        }
        let end = utf8_safe_prefix(rest, limit);
        out.push_str(&rest[..end]);
        out.push_str("\r\n");
        rest = &rest[end..];
        first = false;
    }
    if first {
        out.push_str("\r\n");
    }
}

/// Length of the longest prefix of `value` that fits in `max_bytes` without
/// splitting a UTF-8 sequence. Always returns at least one character so folding
/// terminates even when a single character exceeds the budget.
fn utf8_safe_prefix(value: &str, max_bytes: usize) -> usize {
    if value.len() <= max_bytes {
        return value.len();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        return value
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(0);
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, date: &str, title: &str) -> CalendarEvent {
        CalendarEvent {
            id: id.to_string(),
            date: date
                .parse()
                .unwrap(),
            title: title.to_string(),
            series_title: None,
            season_number: None,
            episode_number: None,
            updated_at: "2026-01-01T00:00:00"
                .parse()
                .unwrap(),
        }
    }

    #[test]
    fn serializes_all_day_events_with_exclusive_end_date() {
        let ics = serialize_ics(&[event("movie-id", "2026-08-06", "Dune")]);
        assert!(
            ics.contains(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Remux//Calendar//EN\r\nCALSCALE:GREGORIAN\r\n"
            ),
            "missing calendar header: {ics}"
        );
        assert!(ics.contains("UID:movie-id@remux\r\n"), "{ics}");
        // A one-day all-day event must end on the next date, not the same one.
        assert!(
            ics.contains(
                "DTSTART;VALUE=DATE:20260806\r\nDTEND;VALUE=DATE:20260807\r\n"
            ),
            "{ics}"
        );
        assert!(ics.ends_with("END:VCALENDAR\r\n"), "{ics}");
    }

    #[test]
    fn episode_summary_joins_series_code_and_title() {
        let mut episode = event("ep", "2026-08-07", "Épisode final");
        episode.series_title = Some("Série".into());
        episode.season_number = Some(2);
        episode.episode_number = Some(3);
        let ics = serialize_ics(&[episode]);
        assert!(
            ics.contains("SUMMARY:Série - S02E03 - Épisode final\r\n"),
            "{ics}"
        );
    }

    #[test]
    fn episode_summary_omits_missing_components() {
        let mut episode = event("ep", "2026-08-07", "Pilot");
        episode.series_title = Some("Show".into());
        // Half-known numbering must not render "S02E00".
        episode.season_number = Some(2);
        let ics = serialize_ics(&[episode]);
        assert!(ics.contains("SUMMARY:Show - Pilot\r\n"), "{ics}");
    }

    #[test]
    fn escapes_special_characters_in_text_values() {
        let ics =
            serialize_ics(&[event("id", "2026-08-06", "L'été, puis; l'hiver\\fin")]);
        assert!(
            ics.contains("SUMMARY:L'été\\, puis\\; l'hiver\\\\fin\r\n"),
            "{ics}"
        );
    }

    #[test]
    fn strips_forbidden_controls_and_escapes_newlines() {
        let forbidden: String = (0u8..=0x08)
            .chain([0x0b, 0x0c])
            .chain(0x0e..=0x1f)
            .chain([0x7f])
            .map(|b| b as char)
            .collect();
        let mut source = event("i\u{0}d", "2026-01-02", "");
        source.title = format!("before{forbidden}\tmid\r\nnext\rafter\nend");
        let ics = serialize_ics(&[source]);

        for byte in (0u8..=0x08)
            .chain([0x0b, 0x0c])
            .chain(0x0e..=0x1f)
            .chain([0x7f])
        {
            assert!(
                !ics.as_bytes()
                    .contains(&byte),
                "forbidden control byte {byte:#04x} survived: {ics:?}"
            );
        }
        assert!(ics.contains("UID:id@remux\r\n"), "{ics:?}");
        // TAB is legal in TEXT and must survive; CRLF/CR/LF all collapse to \n.
        assert!(
            ics.contains("SUMMARY:before\tmid\\nnext\\nafter\\nend\r\n"),
            "{ics:?}"
        );
    }

    #[test]
    fn folds_long_lines_at_75_octets_without_splitting_utf8() {
        let mut long = event("folded", "2026-01-02", "");
        long.title = "é🎬".repeat(30);
        let ics = serialize_ics(&[long]);

        let mut saw_continuation = false;
        for line in ics
            .split("\r\n")
            .filter(|l| !l.is_empty())
        {
            assert!(
                line.len() <= LINE_OCTET_LIMIT,
                "line has {} octets: {line:?}",
                line.len()
            );
            if line.starts_with(' ') {
                saw_continuation = true;
            }
        }
        assert!(saw_continuation, "long summary was not folded: {ics}");
        // Folding must be reversible: unfolding restores the original summary.
        let unfolded = ics.replace("\r\n ", "");
        assert!(
            unfolded.contains(&format!("SUMMARY:{}", "é🎬".repeat(30))),
            "unfolded summary lost content: {unfolded}"
        );
    }

    #[test]
    fn output_contains_no_bare_line_feeds() {
        let ics = serialize_ics(&[event("id", "2026-01-02", "Title")]);
        assert!(
            !ics.replace("\r\n", "")
                .contains('\n'),
            "bare LF in output: {ics:?}"
        );
    }

    #[test]
    fn identical_input_serializes_byte_identically() {
        let source = event("stable", "2026-05-01", "Stable");
        assert_eq!(
            serialize_ics(std::slice::from_ref(&source)),
            serialize_ics(&[source.clone()]),
            "serialization is not deterministic"
        );
        // DTSTAMP must come from updated_at, not the current clock.
        assert!(serialize_ics(&[source]).contains("DTSTAMP:20260101T000000Z\r\n"));
    }
}
