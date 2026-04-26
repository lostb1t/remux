use crate::db::ProgramKind;

/// Parsed channel from an M3U/M3U8 playlist.
#[derive(Debug, Clone, Default)]
pub struct M3uChannel {
    /// tvg-id attribute (used for EPG matching)
    pub tvg_id: Option<String>,
    /// tvg-name or the display name after the comma on #EXTINF
    pub name: String,
    /// tvg-logo URL
    pub logo: Option<String>,
    /// group-title attribute
    pub group: Option<String>,
    /// tvg-chno or ch-number attribute
    pub channel_number: Option<i64>,
    /// The stream URL (the line following #EXTINF)
    pub url: String,
    /// Derived from group-title (and Xtream category for Xtream sources)
    pub program_kind: Option<ProgramKind>,
}

/// Parse an M3U playlist string into a list of channels.
/// Ignores any entry that has no valid stream URL.
pub fn parse_m3u(content: &str) -> Vec<M3uChannel> {
    let mut channels = Vec::new();
    let mut pending: Option<M3uChannel> = None;

    for line in content.lines() {
        let line = line.trim();

        if line.starts_with("#EXTINF") {
            // Extract the duration/attributes part and the display name.
            // Format: #EXTINF:<duration> [key="val" ...],<name>
            let after = line.strip_prefix("#EXTINF:").unwrap_or(line);

            // Split on the first comma to separate attributes from name.
            let (attrs_part, name_part) = match after.find(',') {
                Some(idx) => (&after[..idx], after[idx + 1..].trim()),
                None => (after, ""),
            };

            let name = name_part.to_string();

            // Parse key="value" attributes from the attrs_part.
            let tvg_id = extract_attr(attrs_part, "tvg-id");
            let logo = extract_attr(attrs_part, "tvg-logo");
            let group = extract_attr(attrs_part, "group-title");
            let tvg_name = extract_attr(attrs_part, "tvg-name");
            let channel_number = extract_attr(attrs_part, "tvg-chno")
                .or_else(|| extract_attr(attrs_part, "ch-number"))
                .and_then(|s| s.parse::<i64>().ok());

            let program_kind = group.as_deref().and_then(super::parse_program_kind);
            pending = Some(M3uChannel {
                tvg_id,
                name: tvg_name.unwrap_or(name),
                logo,
                group,
                channel_number,
                url: String::new(),
                program_kind,
            });
        } else if !line.is_empty() && !line.starts_with('#') {
            // This is a stream URL line.
            if let Some(mut ch) = pending.take() {
                ch.url = line.to_string();
                channels.push(ch);
            }
        }
    }

    channels
}

/// Extract a `key="value"` attribute from an #EXTINF attribute string.
fn extract_attr(s: &str, key: &str) -> Option<String> {
    // Try key="..." first, then key=...
    let search_quoted = format!("{}=\"", key);
    if let Some(start) = s.find(search_quoted.as_str()) {
        let after = &s[start + search_quoted.len()..];
        let end = after.find('"').unwrap_or(after.len());
        let val = after[..end].trim();
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }

    let search_unquoted = format!("{}=", key);
    if let Some(start) = s.find(search_unquoted.as_str()) {
        let after = &s[start + search_unquoted.len()..];
        // Value ends at next space or end of string
        let end = after.find(' ').unwrap_or(after.len());
        let val = after[..end].trim().trim_matches('"');
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_m3u() {
        let m3u = r#"#EXTM3U
#EXTINF:-1 tvg-id="ch1" tvg-name="Channel 1" tvg-logo="http://logo/1.png" group-title="News",Channel 1
http://stream/1
#EXTINF:-1 tvg-id="ch2" tvg-chno="2",Channel 2
http://stream/2
"#;
        let channels = parse_m3u(m3u);
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].tvg_id.as_deref(), Some("ch1"));
        assert_eq!(channels[0].name, "Channel 1");
        assert_eq!(channels[0].url, "http://stream/1");
        assert_eq!(channels[1].channel_number, Some(2));
    }
}
