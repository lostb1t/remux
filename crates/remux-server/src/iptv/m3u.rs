use crate::db::ProgramKind;
use anyhow::Result;
use futures::TryStreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::io::StreamReader;

/// Parsed channel from an M3U/M3U8 playlist.
#[derive(Debug, Clone, Default)]
pub struct M3uChannel {
    /// tvg-id attribute (used for EPG matching)
    pub tvg_id: Option<String>,
    /// tvg-name or the display name after the comma on #EXTINF
    pub name: String,
    /// tvg-logo URL
    pub logo: Option<String>,
    /// group-title attribute (first segment if semicolon-delimited)
    pub group: Option<String>,
    /// tvg-chno or ch-number attribute
    pub channel_number: Option<i64>,
    /// The stream URL (the line following #EXTINF)
    pub url: String,
    /// Derived from group-title (and Xtream category for Xtream sources)
    pub program_kind: Option<ProgramKind>,
    /// tvg-language attribute
    pub language: Option<String>,
    /// Catchup type (e.g. "default", "append", "shift", "flussonic")
    pub catchup: Option<String>,
    /// Number of days of catchup content available
    pub catchup_days: Option<i64>,
    /// Catchup URL template (`catchup-source` attribute)
    pub catchup_source: Option<String>,
}

/// Stream-parse an M3U playlist from a reqwest response without loading the
/// entire body into memory.
pub async fn parse_m3u_stream(resp: reqwest::Response) -> Result<Vec<M3uChannel>> {
    let stream = resp
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let mut lines = BufReader::new(StreamReader::new(stream)).lines();

    let mut channels = Vec::new();
    let mut pending: Option<M3uChannel> = None;

    while let Some(line) = lines
        .next_line()
        .await?
    {
        let line = line
            .trim()
            .to_string();
        if line.starts_with("#EXTINF") {
            pending = Some(parse_extinf(&line));
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(mut ch) = pending.take() {
                ch.url = line;
                channels.push(ch);
            }
        }
    }

    Ok(channels)
}

fn parse_extinf(line: &str) -> M3uChannel {
    let after = line
        .strip_prefix("#EXTINF:")
        .unwrap_or(line);
    let (attrs_part, name_part) = match after.find(',') {
        Some(idx) => (&after[..idx], after[idx + 1..].trim()),
        None => (after, ""),
    };
    let tvg_id = extract_attr(attrs_part, "tvg-id");
    let logo = extract_attr(attrs_part, "tvg-logo");
    // group-title may be semicolon-delimited; use the first segment as the primary group.
    let group_raw = extract_attr(attrs_part, "group-title");
    let group = group_raw
        .as_deref()
        .and_then(|g| {
            g.split(';')
                .next()
        })
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.trim()
                .to_string()
        });
    let tvg_name = extract_attr(attrs_part, "tvg-name");
    let channel_number = extract_attr(attrs_part, "tvg-chno")
        .or_else(|| extract_attr(attrs_part, "ch-number"))
        .and_then(|s| {
            s.parse::<i64>()
                .ok()
        });
    let language = extract_attr(attrs_part, "tvg-language");
    let catchup = extract_attr(attrs_part, "catchup");
    let catchup_days = extract_attr(attrs_part, "catchup-days").and_then(|s| {
        s.parse::<i64>()
            .ok()
    });
    let catchup_source = extract_attr(attrs_part, "catchup-source");
    let program_kind = group
        .as_deref()
        .and_then(super::parse_program_kind);
    M3uChannel {
        tvg_id,
        name: tvg_name.unwrap_or_else(|| name_part.to_string()),
        logo,
        group,
        channel_number,
        url: String::new(),
        program_kind,
        language,
        catchup,
        catchup_days,
        catchup_source,
    }
}

/// Extract a `key="value"` attribute from an #EXTINF attribute string.
fn extract_attr(s: &str, key: &str) -> Option<String> {
    // Try key="..." first, then key=...
    let search_quoted = format!("{}=\"", key);
    if let Some(start) = s.find(search_quoted.as_str()) {
        let after = &s[start + search_quoted.len()..];
        let end = after
            .find('"')
            .unwrap_or(after.len());
        let val = after[..end].trim();
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }

    let search_unquoted = format!("{}=", key);
    if let Some(start) = s.find(search_unquoted.as_str()) {
        let after = &s[start + search_unquoted.len()..];
        // Value ends at next space or end of string
        let end = after
            .find(' ')
            .unwrap_or(after.len());
        let val = after[..end]
            .trim()
            .trim_matches('"');
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parse_basic_m3u() {
        let m3u = "#EXTM3U\n\
            #EXTINF:-1 tvg-id=\"ch1\" tvg-name=\"Channel 1\" tvg-logo=\"http://logo/1.png\" group-title=\"News\",Channel 1\n\
            http://stream/1\n\
            #EXTINF:-1 tvg-id=\"ch2\" tvg-chno=\"2\",Channel 2\n\
            http://stream/2\n";
        let resp = reqwest::Response::from(http::Response::new(
            m3u.as_bytes()
                .to_vec(),
        ));
        let channels = parse_m3u_stream(resp)
            .await
            .unwrap();
        assert_eq!(channels.len(), 2);
        assert_eq!(
            channels[0]
                .tvg_id
                .as_deref(),
            Some("ch1")
        );
        assert_eq!(channels[0].name, "Channel 1");
        assert_eq!(channels[0].url, "http://stream/1");
        assert_eq!(channels[1].channel_number, Some(2));
    }
}
