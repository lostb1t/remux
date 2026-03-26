use anyhow::{Context, Result};
use chrono::NaiveDateTime;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

/// A single programme entry from XMLTV.
#[derive(Debug, Clone, Default)]
pub struct EpgProgram {
    /// Matches `M3uChannel::tvg_id` / XMLTV channel id
    pub channel_id: String,
    pub title: String,
    pub description: Option<String>,
    pub start: Option<NaiveDateTime>,
    pub end: Option<NaiveDateTime>,
}

/// Parse XMLTV content. Returns a list of programs.
/// Large files are parsed with a SAX-style reader to avoid loading everything into memory.
pub fn parse_xmltv(content: &str) -> Result<Vec<EpgProgram>> {
    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(true);

    let mut programs: Vec<EpgProgram> = Vec::new();
    let mut current: Option<EpgProgram> = None;
    let mut in_title = false;
    let mut in_desc = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"programme" => {
                    let mut prog = EpgProgram::default();
                    for attr in e.attributes().flatten() {
                        let key = attr.key.as_ref();
                        let val = String::from_utf8_lossy(&attr.value).into_owned();
                        match key {
                            b"channel" => prog.channel_id = val,
                            b"start" => prog.start = parse_xmltv_datetime(&val),
                            b"stop" => prog.end = parse_xmltv_datetime(&val),
                            _ => {}
                        }
                    }
                    current = Some(prog);
                }
                b"title" => {
                    if current.is_some() {
                        in_title = true;
                    }
                }
                b"desc" => {
                    if current.is_some() {
                        in_desc = true;
                    }
                }
                _ => {}
            },
            Ok(Event::Text(ref e)) => {
                if let Some(ref mut prog) = current {
                    let text = e.unescape().unwrap_or_default().into_owned();
                    if in_title {
                        prog.title = text;
                    } else if in_desc {
                        prog.description = Some(text);
                    }
                }
            }
            Ok(Event::End(ref e)) => match e.name().as_ref() {
                b"title" => in_title = false,
                b"desc" => in_desc = false,
                b"programme" => {
                    if let Some(prog) = current.take() {
                        if !prog.channel_id.is_empty() && !prog.title.is_empty() {
                            programs.push(prog);
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("XMLTV parse error: {}", e)),
            _ => {}
        }
    }

    Ok(programs)
}

/// Parse XMLTV datetime format: `20240101120000 +0000` or `20240101120000`
fn parse_xmltv_datetime(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    // Strip timezone offset (everything after a space)
    let dt_part = s.split_whitespace().next()?;
    // Try common formats
    NaiveDateTime::parse_from_str(dt_part, "%Y%m%d%H%M%S")
        .or_else(|_| NaiveDateTime::parse_from_str(dt_part, "%Y%m%d%H%M"))
        .ok()
}
