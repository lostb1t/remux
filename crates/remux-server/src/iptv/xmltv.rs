use anyhow::Result;
use chrono::NaiveDateTime;
use quick_xml::{events::Event, reader::Reader};
use tracing::debug;

use super::parse_program_kind;
use crate::db::ProgramKind; // used by EpgProgram

/// A single programme entry from XMLTV.
#[derive(Debug, Clone, Default)]
pub struct EpgProgram {
    /// Matches `M3uChannel::tvg_id` / XMLTV channel id
    pub channel_id: String,
    pub title: String,
    pub description: Option<String>,
    pub start: Option<NaiveDateTime>,
    pub end: Option<NaiveDateTime>,
    pub program_kind: Option<ProgramKind>,
    /// Thumbnail URL from `<icon src="..."/>` inside `<programme>`
    pub poster: Option<String>,
}

/// Appends a resolved `&amp;`-style or numeric entity reference to `out`.
/// quick-xml delivers entity references as their own events, separate from
/// the surrounding text.
fn push_entity_ref(out: &mut String, r: &quick_xml::events::BytesRef<'_>) {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        out.push(c);
        return;
    }
    match &**r {
        "amp" => out.push('&'),
        "lt" => out.push('<'),
        "gt" => out.push('>'),
        "quot" => out.push('"'),
        "apos" => out.push('\''),
        _ => {}
    }
}

/// Parse XMLTV from any buffered reader, calling `on_program` for each
/// complete programme element. The full XML is never held in memory.
pub fn parse_xmltv<R: std::io::BufRead, F: FnMut(EpgProgram)>(
    input: R,
    mut on_program: F,
) -> Result<()> {
    let mut reader = Reader::from_reader(input);

    let mut current: Option<EpgProgram> = None;
    let mut in_title = false;
    let mut in_desc = false;
    let mut in_category = false;
    let mut text = String::new();
    let mut buf = Vec::new();
    let mut total = 0usize;
    let mut with_kind = 0usize;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e
                .name()
                .as_ref()
            {
                "programme" => {
                    let mut prog = EpgProgram::default();
                    for attr in e
                        .attributes()
                        .flatten()
                    {
                        let key = attr
                            .key
                            .as_ref();
                        let val = attr
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map(|v| v.into_owned())
                            .unwrap_or_default();
                        match key {
                            "channel" => prog.channel_id = val,
                            "start" => prog.start = parse_xmltv_datetime(&val),
                            "stop" => prog.end = parse_xmltv_datetime(&val),
                            _ => {}
                        }
                    }
                    current = Some(prog);
                }
                "title" => {
                    if current.is_some() {
                        in_title = true;
                        text.clear();
                    }
                }
                "desc" => {
                    if current.is_some() {
                        in_desc = true;
                        text.clear();
                    }
                }
                "category" => {
                    if current.is_some() {
                        in_category = true;
                        text.clear();
                    }
                }
                _ => {}
            },
            Ok(Event::Text(ref e)) => {
                if in_title || in_desc || in_category {
                    text.push_str(&e.xml_content(quick_xml::XmlVersion::Implicit1_0));
                }
            }
            Ok(Event::GeneralRef(ref r)) => {
                if in_title || in_desc || in_category {
                    push_entity_ref(&mut text, r);
                }
            }
            Ok(Event::End(ref e)) => match e
                .name()
                .as_ref()
            {
                "title" => {
                    if in_title {
                        if let Some(ref mut prog) = current {
                            prog.title = text
                                .trim()
                                .to_string();
                        }
                    }
                    in_title = false;
                }
                "desc" => {
                    if in_desc {
                        if let Some(ref mut prog) = current {
                            prog.description = Some(
                                text.trim()
                                    .to_string(),
                            );
                        }
                    }
                    in_desc = false;
                }
                "category" => {
                    if in_category {
                        if let Some(ref mut prog) = current {
                            if prog
                                .program_kind
                                .is_none()
                            {
                                let cat = text.trim();
                                let kind = parse_program_kind(cat);
                                debug!(category = %cat, matched = ?kind, "xmltv category");
                                prog.program_kind = kind;
                            }
                        }
                    }
                    in_category = false;
                }
                "programme" => {
                    if let Some(prog) = current.take() {
                        if !prog
                            .channel_id
                            .is_empty()
                            && !prog
                                .title
                                .is_empty()
                        {
                            if prog
                                .program_kind
                                .is_some()
                            {
                                with_kind += 1;
                            }
                            total += 1;
                            on_program(prog);
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(ref e)) => {
                if e.name()
                    .as_ref()
                    == "icon"
                {
                    if let Some(ref mut prog) = current {
                        if prog
                            .poster
                            .is_none()
                        {
                            prog.poster = e
                                .attributes()
                                .flatten()
                                .find_map(|a| {
                                    if a.key
                                        .as_ref()
                                        == "src"
                                    {
                                        let url = a
                                            .normalized_value(
                                                quick_xml::XmlVersion::Implicit1_0,
                                            )
                                            .map(|v| v.into_owned())
                                            .unwrap_or_default();
                                        if !url.is_empty() { Some(url) } else { None }
                                    } else {
                                        None
                                    }
                                });
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("XMLTV parse error: {}", e)),
            _ => {}
        }
    }

    debug!(total, with_program_kind = with_kind, "xmltv parse complete");
    Ok(())
}

/// Parse XMLTV datetime format: `20240101120000 +0000` or `20240101120000`
fn parse_xmltv_datetime(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    // Strip timezone offset (everything after a space)
    let dt_part = s
        .split_whitespace()
        .next()?;
    // Try common formats
    NaiveDateTime::parse_from_str(dt_part, "%Y%m%d%H%M%S")
        .or_else(|_| NaiveDateTime::parse_from_str(dt_part, "%Y%m%d%H%M"))
        .ok()
}
