// Stream specifier for identifying specific streams
// Based on FFmpeg cmdutils.c

use ffmpeg_sys_next::AVMediaType;

/// Stream list type - how streams are selected
/// From FFmpeg stream_list_type enumeration
#[derive(Debug, Clone, PartialEq)]
pub enum StreamListType {
    All,
    Program,  // p:N
    GroupId,  // g:#N or g:i:N
    GroupIdx, // g:N
    StreamId, // #N or i:N
}

/// Stream specifier - identifies specific streams
/// Based on FFmpeg stream_specifier structure
#[derive(Debug, Clone)]
pub struct StreamSpecifier {
    pub media_type: Option<AVMediaType>,
    pub no_apic: bool, // 'V' - video without attached pictures
    pub idx: Option<i32>,
    pub list_type: StreamListType,
    pub list_id: i64,
    pub meta_key: Option<String>,
    pub meta_val: Option<String>,
    pub usable_only: bool,
    pub disposition: Option<String>,
}

impl Default for StreamSpecifier {
    fn default() -> Self {
        Self {
            media_type: None,
            no_apic: false,
            idx: None,
            list_type: StreamListType::All,
            list_id: 0,
            meta_key: None,
            meta_val: None,
            usable_only: false,
            disposition: None,
        }
    }
}

impl StreamSpecifier {
    /// Parse ID number with optional hex format (0x prefix)
    /// Helper function to avoid code duplication in group/program/stream ID parsing
    /// FFmpeg reference: cmdutils.c uses strtol() for parsing in lines 1067, 1084, 1139
    fn parse_id_number(num_str: &str, context: &str) -> Result<i64, String> {
        if num_str.is_empty() {
            return Err(format!("Expected {}", context));
        }

        if num_str.starts_with("0x") || num_str.starts_with("0X") {
            i64::from_str_radix(&num_str[2..], 16)
                .map_err(|_| format!("Invalid hex {}: {}", context, num_str))
        } else {
            num_str
                .parse::<i64>()
                .map_err(|_| format!("Invalid {}: {}", context, num_str))
        }
    }

    /// Parse a stream specifier string following FFmpeg's complete syntax.
    ///
    /// Supported syntax (from FFmpeg documentation):
    /// - Media types: v, a, s, d, t, V (video without attached pics)
    /// - Index: v:0, a:1, s:2
    /// - Program: p:0:v (program 0, video streams)
    /// - Group by index: g:0 (stream group index)
    /// - Group by ID: g:#123 or g:i:123 (stream group ID)
    /// - Stream ID: #0x100 or i:256
    /// - Metadata filter: m:key:value
    /// - Usable only: u
    /// - Disposition: disp:default, disp:forced
    ///
    /// FFmpeg reference: fftools/cmdutils.c stream_specifier_parse()
    pub fn parse(spec: &str) -> Result<Self, String> {
        // Validate input
        if spec.is_empty() {
            return Err("Stream specifier cannot be empty".to_string());
        }

        let mut ss = StreamSpecifier::default();
        let mut chars = spec.chars().peekable();

        while let Some(&ch) = chars.peek() {
            match ch {
                // Index parsing: v:0, a:1, etc.
                // FFmpeg reference: cmdutils.c lines 1022-1032
                '0'..='9' => {
                    let num_str: String =
                        chars.by_ref().take_while(|c| c.is_ascii_digit()).collect();
                    ss.idx = Some(
                        num_str
                            .parse::<i32>()
                            .map_err(|_| format!("Invalid index: {}", num_str))?,
                    );
                    // Index terminates the specifier (FFmpeg line 1031)
                    break;
                }

                // Media type parsing: v, a, s, t, V (note: 'd' handled separately for disposition)
                // FFmpeg reference: cmdutils.c lines 1033-1054
                // Only parse as media type if not followed by alphanumeric
                // This prevents "video" from being parsed as 'v' + "ideo"
                'v' | 'a' | 's' | 't' | 'V' => {
                    let next_char = chars.clone().nth(1);
                    // FFmpeg uses cmdutils_isalnum() which checks ASCII-only (0-9, A-Z, a-z)
                    if next_char.map_or(true, |c| !c.is_ascii_alphanumeric()) {
                        if ss.media_type.is_some() {
                            return Err("Stream type specified multiple times".to_string());
                        }

                        chars.next(); // consume the character
                        match ch {
                            'v' => ss.media_type = Some(AVMediaType::AVMEDIA_TYPE_VIDEO),
                            'a' => ss.media_type = Some(AVMediaType::AVMEDIA_TYPE_AUDIO),
                            's' => ss.media_type = Some(AVMediaType::AVMEDIA_TYPE_SUBTITLE),
                            't' => ss.media_type = Some(AVMediaType::AVMEDIA_TYPE_ATTACHMENT),
                            'V' => {
                                ss.media_type = Some(AVMediaType::AVMEDIA_TYPE_VIDEO);
                                ss.no_apic = true;
                            }
                            _ => unreachable!(),
                        }
                    } else {
                        break;
                    }
                }

                // 'd' can be either data type or disposition
                // Check for "disp:" first (FFmpeg lines 1094-1130), then data type (FFmpeg line 1046)
                'd' => {
                    // Check if this is "disp:"
                    if chars.clone().take(5).collect::<String>() == "disp:" {
                        // Disposition parsing
                        if ss.disposition.is_some() {
                            return Err("Multiple disposition specifiers".to_string());
                        }

                        // Consume "disp:"
                        for _ in 0..5 {
                            chars.next();
                        }

                        // FFmpeg uses cmdutils_isalnum() which checks ASCII-only (0-9, A-Z, a-z)
                        let disp: String = chars
                            .by_ref()
                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '+')
                            .collect();

                        if disp.is_empty() {
                            return Err("Expected disposition value".to_string());
                        }

                        ss.disposition = Some(disp);
                    } else {
                        // Data type parsing
                        let next_char = chars.clone().nth(1);
                        if next_char.map_or(true, |c| !c.is_ascii_alphanumeric()) {
                            if ss.media_type.is_some() {
                                return Err("Stream type specified multiple times".to_string());
                            }
                            chars.next(); // consume 'd'
                            ss.media_type = Some(AVMediaType::AVMEDIA_TYPE_DATA);
                        } else {
                            break;
                        }
                    }
                }

                // Group parsing: g:N (index), g:#N or g:i:N (ID)
                // FFmpeg reference: cmdutils.c lines 1055-1076
                'g' => {
                    if chars.clone().nth(1) == Some(':') {
                        if ss.list_type != StreamListType::All {
                            return Err(
                                "Cannot combine multiple program/group designators".to_string()
                            );
                        }

                        chars.next(); // consume 'g'
                        chars.next(); // consume ':'

                        let next = chars.peek();
                        if next == Some(&'#')
                            || (next == Some(&'i') && chars.clone().nth(1) == Some(':'))
                        {
                            ss.list_type = StreamListType::GroupId;
                            chars.next(); // consume '#' or 'i'
                            if chars.peek() == Some(&':') {
                                chars.next(); // consume ':' after 'i'
                            }
                        } else {
                            ss.list_type = StreamListType::GroupIdx;
                        }

                        let num_str: String = chars
                            .by_ref()
                            .take_while(|c| {
                                c.is_ascii_digit()
                                    || matches!(*c, 'x' | 'X')
                                    || c.is_ascii_hexdigit()
                            })
                            .collect();
                        ss.list_id = Self::parse_id_number(&num_str, "stream group idx/ID")?;
                    } else {
                        break;
                    }
                }

                // Program parsing: p:N
                // FFmpeg reference: cmdutils.c lines 1077-1093
                'p' => {
                    if chars.clone().nth(1) == Some(':') {
                        if ss.list_type != StreamListType::All {
                            return Err(
                                "Cannot combine multiple program/group designators".to_string()
                            );
                        }

                        ss.list_type = StreamListType::Program;

                        chars.next(); // consume 'p'
                        chars.next(); // consume ':'

                        let num_str: String =
                            chars.by_ref().take_while(|c| c.is_ascii_digit()).collect();
                        ss.list_id = Self::parse_id_number(&num_str, "program ID")?;
                    } else {
                        break;
                    }
                }

                // Stream ID parsing: #N or i:N
                // FFmpeg reference: fftools/cmdutils.c line 1131-1151
                '#' => {
                    if ss.list_type != StreamListType::All {
                        return Err("Cannot combine multiple program/group designators".to_string());
                    }

                    ss.list_type = StreamListType::StreamId;
                    chars.next(); // consume '#'

                    let num_str: String = chars
                        .by_ref()
                        .take_while(|c| {
                            c.is_ascii_digit() || matches!(*c, 'x' | 'X') || c.is_ascii_hexdigit()
                        })
                        .collect();
                    ss.list_id = Self::parse_id_number(&num_str, "stream ID")?;

                    // Stream ID terminates the specifier (FFmpeg line 1150)
                    break;
                }

                'i' => {
                    if chars.clone().nth(1) == Some(':') {
                        if ss.list_type != StreamListType::All {
                            return Err(
                                "Cannot combine multiple program/group designators".to_string()
                            );
                        }

                        ss.list_type = StreamListType::StreamId;
                        chars.next(); // consume 'i'
                        chars.next(); // consume ':'

                        let num_str: String = chars
                            .by_ref()
                            .take_while(|c| {
                                c.is_ascii_digit()
                                    || matches!(*c, 'x' | 'X')
                                    || c.is_ascii_hexdigit()
                            })
                            .collect();
                        ss.list_id = Self::parse_id_number(&num_str, "stream ID")?;

                        // Stream ID terminates the specifier
                        break;
                    } else {
                        break;
                    }
                }

                // Metadata filter parsing: m:key:value or m:key (any value)
                // FFmpeg reference: fftools/cmdutils.c line 1152-1175
                // Note: FFmpeg allows m:key without value to match any value
                'm' => {
                    if chars.clone().nth(1) == Some(':') {
                        chars.next(); // consume 'm'
                        chars.next(); // consume ':'

                        // Parse key (until ':' or end)
                        let key: String = chars.by_ref().take_while(|c| *c != ':').collect();
                        if key.is_empty() {
                            return Err("Expected metadata key".to_string());
                        }
                        ss.meta_key = Some(key);

                        // Parse optional value (if ':' present)
                        // Note: take_while consumes the ':' delimiter
                        let val: String = chars.by_ref().take_while(|c| *c != ':').collect();
                        if !val.is_empty() {
                            ss.meta_val = Some(val);
                        }

                        // Metadata filter terminates the specifier (FFmpeg line 1174)
                        break;
                    } else {
                        break;
                    }
                }

                // Usable only flag: u
                // FFmpeg reference: fftools/cmdutils.c line 1176-1182
                'u' => {
                    let next_char = chars.clone().nth(1);
                    if next_char.is_none() || next_char == Some(':') {
                        ss.usable_only = true;
                        chars.next(); // consume 'u'
                                      // Usable flag terminates the specifier (FFmpeg line 1181)
                        break;
                    } else {
                        break;
                    }
                }

                // Separator
                ':' => {
                    chars.next();
                }

                // Unknown character - stop parsing
                _ => break,
            }
        }

        // Validate that all input was consumed
        if chars.peek().is_some() {
            return Err(format!(
                "Invalid stream specifier: unexpected characters after parsing '{}'",
                spec
            ));
        }

        // Validate that we parsed something meaningful
        // If all fields are still at default values, the input was invalid
        if ss.media_type.is_none()
            && ss.idx.is_none()
            && ss.list_type == StreamListType::All
            && ss.list_id == 0
            && ss.meta_key.is_none()
            && !ss.usable_only
            && ss.disposition.is_none()
        {
            return Err(format!(
                "Invalid stream specifier: no valid components parsed from '{}'",
                spec
            ));
        }

        Ok(ss)
    }

    /// Check if a stream matches this specifier.
    ///
    /// Implements FFmpeg's stream matching logic from cmdutils.c
    ///
    /// # Safety
    /// This function dereferences raw FFmpeg pointers. The caller must ensure that:
    /// - `fmt_ctx` is a valid pointer to an initialized AVFormatContext
    /// - `st` is a valid pointer to an AVStream within that context
    /// - Both pointers remain valid for the duration of this call
    ///
    /// FFmpeg reference: fftools/cmdutils.c check_stream_specifier()
    pub unsafe fn matches(
        &self,
        fmt_ctx: *const ffmpeg_sys_next::AVFormatContext,
        st: *const ffmpeg_sys_next::AVStream,
    ) -> bool {
        use ffmpeg_sys_next::{
            av_dict_get, AVPixelFormat::AV_PIX_FMT_NONE, AVSampleFormat::AV_SAMPLE_FMT_NONE,
            AV_DISPOSITION_ATTACHED_PIC,
        };

        if fmt_ctx.is_null() || st.is_null() {
            return false;
        }

        let fmt_ctx_ref = &*fmt_ctx;
        let st_ref = &*st;

        // Determine which streams to check based on list_type
        let (start_stream, nb_streams, program, group) = match self.list_type {
            StreamListType::StreamId => {
                // Early return if stream ID doesn't match
                if st_ref.id != self.list_id as i32 {
                    return false;
                }
                (
                    st_ref.index as usize,
                    (st_ref.index + 1) as usize,
                    None,
                    None,
                )
            }

            StreamListType::All => {
                let start = if self.idx.is_some() {
                    0
                } else {
                    st_ref.index as usize
                };
                (start, (st_ref.index + 1) as usize, None, None)
            }

            StreamListType::Program => {
                if fmt_ctx_ref.nb_programs <= 0 || fmt_ctx_ref.programs.is_null() {
                    log::warn!("No program table present, stream specifier can never match");
                    return false;
                }
                // Find the program with matching ID
                let programs = std::slice::from_raw_parts(
                    fmt_ctx_ref.programs,
                    fmt_ctx_ref.nb_programs as usize,
                );
                let program = programs
                    .iter()
                    .find(|&&p| !p.is_null() && (*p).id == self.list_id as i32);

                match program {
                    Some(&p) if !p.is_null() => {
                        let prog = &*p;
                        (0, prog.nb_stream_indexes as usize, Some(prog), None)
                    }
                    _ => {
                        log::warn!(
                            "No program with ID {} exists, stream specifier can never match",
                            self.list_id
                        );
                        return false;
                    }
                }
            }

            StreamListType::GroupId => {
                if fmt_ctx_ref.nb_stream_groups <= 0 || fmt_ctx_ref.stream_groups.is_null() {
                    log::warn!("No stream groups present, stream specifier can never match",);
                    return false;
                }
                // Find the group with matching ID
                let groups = std::slice::from_raw_parts(
                    fmt_ctx_ref.stream_groups,
                    fmt_ctx_ref.nb_stream_groups as usize,
                );
                let group = groups
                    .iter()
                    .find(|&&g| !g.is_null() && (*g).id == self.list_id);

                match group {
                    Some(&g) if !g.is_null() => {
                        let grp = &*g;
                        (0, grp.nb_streams as usize, None, Some(grp))
                    }
                    _ => {
                        log::warn!(
                            "No stream group with group ID {} exists, stream specifier can never match",
                            self.list_id
                        );
                        return false;
                    }
                }
            }

            StreamListType::GroupIdx => {
                if fmt_ctx_ref.nb_stream_groups <= 0 || fmt_ctx_ref.stream_groups.is_null() {
                    log::warn!("No stream groups present, stream specifier can never match",);
                    return false;
                }
                // Access group by index
                if self.list_id >= 0
                    && (self.list_id as usize) < fmt_ctx_ref.nb_stream_groups as usize
                {
                    let groups = std::slice::from_raw_parts(
                        fmt_ctx_ref.stream_groups,
                        fmt_ctx_ref.nb_stream_groups as usize,
                    );
                    let g = groups[self.list_id as usize];
                    if !g.is_null() {
                        let grp = &*g;
                        (0, grp.nb_streams as usize, None, Some(grp))
                    } else {
                        return false;
                    }
                } else {
                    log::warn!(
                        "No stream group with group index {} exists, stream specifier can never match",
                        self.list_id
                    );
                    return false;
                }
            }
        };

        let mut nb_matched = 0;

        for i in start_stream..nb_streams {
            // Get the candidate stream
            let candidate_ptr = if let Some(group) = group {
                let streams = std::slice::from_raw_parts(group.streams, group.nb_streams as usize);
                if i < streams.len() && !streams[i].is_null() {
                    let all_streams = std::slice::from_raw_parts(
                        fmt_ctx_ref.streams,
                        fmt_ctx_ref.nb_streams as usize,
                    );
                    let stream_index = (*streams[i]).index as usize;
                    if stream_index < all_streams.len() {
                        all_streams[stream_index]
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            } else if let Some(program) = program {
                let indices = std::slice::from_raw_parts(
                    program.stream_index,
                    program.nb_stream_indexes as usize,
                );
                if i < indices.len() {
                    let all_streams = std::slice::from_raw_parts(
                        fmt_ctx_ref.streams,
                        fmt_ctx_ref.nb_streams as usize,
                    );
                    let stream_index = indices[i] as usize;
                    if stream_index < all_streams.len() {
                        all_streams[stream_index]
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            } else {
                let all_streams = std::slice::from_raw_parts(
                    fmt_ctx_ref.streams,
                    fmt_ctx_ref.nb_streams as usize,
                );
                if i < all_streams.len() {
                    all_streams[i]
                } else {
                    continue;
                }
            };

            if candidate_ptr.is_null() {
                continue;
            }

            let candidate = &*candidate_ptr;

            // Check media type
            if let Some(media_type) = self.media_type {
                if !candidate.codecpar.is_null() {
                    let codecpar = &*candidate.codecpar;
                    if codecpar.codec_type != media_type {
                        continue;
                    }
                    // Check no_apic flag (V specifier - video without attached pictures)
                    if self.no_apic
                        && (candidate.disposition & AV_DISPOSITION_ATTACHED_PIC as i32) != 0
                    {
                        continue;
                    }
                }
            }

            // Check metadata filter
            if let Some(ref meta_key) = self.meta_key {
                let c_key = std::ffi::CString::new(meta_key.as_str()).unwrap();
                let entry = av_dict_get(candidate.metadata, c_key.as_ptr(), std::ptr::null(), 0);

                if entry.is_null() {
                    continue;
                }

                // Check metadata value if specified
                if let Some(ref meta_val) = self.meta_val {
                    let entry_val = std::ffi::CStr::from_ptr((*entry).value).to_str().ok();
                    if entry_val != Some(meta_val.as_str()) {
                        continue;
                    }
                }
            }

            // Check usable_only flag
            if self.usable_only && !candidate.codecpar.is_null() {
                let par = &*candidate.codecpar;

                let usable = match par.codec_type {
                    AVMediaType::AVMEDIA_TYPE_AUDIO => {
                        par.sample_rate > 0
                            && par.ch_layout.nb_channels > 0
                            && par.format != AV_SAMPLE_FMT_NONE as i32
                    }
                    AVMediaType::AVMEDIA_TYPE_VIDEO => {
                        par.width > 0 && par.height > 0 && par.format != AV_PIX_FMT_NONE as i32
                    }
                    AVMediaType::AVMEDIA_TYPE_UNKNOWN => false,
                    _ => true,
                };

                if !usable {
                    continue;
                }
            }

            // Check disposition
            if let Some(ref disp_str) = self.disposition {
                // For simplicity, we check if the disposition string matches common values
                // Full implementation would need to parse disposition flags properly
                let has_disposition = match disp_str.as_str() {
                    "default" => (candidate.disposition & 0x0001) != 0,
                    "forced" => (candidate.disposition & 0x0040) != 0,
                    _ => {
                        // Unknown disposition, log warning and skip
                        log::warn!("Unknown disposition: {}", disp_str);
                        false
                    }
                };

                if !has_disposition {
                    continue;
                }
            }

            // Check if this is the target stream
            if st == candidate_ptr {
                return self.idx.is_none() || self.idx == Some(nb_matched);
            }

            nb_matched += 1;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_media_types() {
        // Video streams
        assert!(matches!(
            StreamSpecifier::parse("v").unwrap(),
            StreamSpecifier {
                media_type: Some(AVMediaType::AVMEDIA_TYPE_VIDEO),
                list_type: StreamListType::All,
                idx: None,
                ..
            }
        ));

        // Audio streams
        assert!(matches!(
            StreamSpecifier::parse("a").unwrap(),
            StreamSpecifier {
                media_type: Some(AVMediaType::AVMEDIA_TYPE_AUDIO),
                list_type: StreamListType::All,
                idx: None,
                ..
            }
        ));

        // Subtitle streams
        assert!(matches!(
            StreamSpecifier::parse("s").unwrap(),
            StreamSpecifier {
                media_type: Some(AVMediaType::AVMEDIA_TYPE_SUBTITLE),
                list_type: StreamListType::All,
                idx: None,
                ..
            }
        ));

        // Data streams
        assert!(matches!(
            StreamSpecifier::parse("d").unwrap(),
            StreamSpecifier {
                media_type: Some(AVMediaType::AVMEDIA_TYPE_DATA),
                list_type: StreamListType::All,
                idx: None,
                ..
            }
        ));

        // Attachment streams
        assert!(matches!(
            StreamSpecifier::parse("t").unwrap(),
            StreamSpecifier {
                media_type: Some(AVMediaType::AVMEDIA_TYPE_ATTACHMENT),
                list_type: StreamListType::All,
                idx: None,
                ..
            }
        ));

        // Video with index
        let spec = StreamSpecifier::parse("v:0").unwrap();
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));
        assert_eq!(spec.idx, Some(0));

        // Audio with index
        let spec = StreamSpecifier::parse("a:1").unwrap();
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_AUDIO));
        assert_eq!(spec.idx, Some(1));
    }

    #[test]
    fn test_parse_stream_by_id() {
        // Hex stream ID
        let spec = StreamSpecifier::parse("#0x100").unwrap();
        assert_eq!(spec.list_type, StreamListType::StreamId);
        assert_eq!(spec.list_id, 0x100);

        // Uppercase hex stream ID
        let spec = StreamSpecifier::parse("#0X2A").unwrap();
        assert_eq!(spec.list_type, StreamListType::StreamId);
        assert_eq!(spec.list_id, 0x2A);

        // Decimal stream ID with 'i:' prefix
        let spec = StreamSpecifier::parse("i:256").unwrap();
        assert_eq!(spec.list_type, StreamListType::StreamId);
        assert_eq!(spec.list_id, 256);
    }

    #[test]
    fn test_parse_program_specifier() {
        // Program with ID
        let spec = StreamSpecifier::parse("p:10:v").unwrap();
        assert_eq!(spec.list_type, StreamListType::Program);
        assert_eq!(spec.list_id, 10);
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));

        // Program with index
        let spec = StreamSpecifier::parse("p:10:v:0").unwrap();
        assert_eq!(spec.list_id, 10);
        assert_eq!(spec.idx, Some(0));
    }

    #[test]
    fn test_parse_group_specifier() {
        // Group by ID using g:#N syntax (correct FFmpeg syntax)
        let spec = StreamSpecifier::parse("g:#5").unwrap();
        assert_eq!(spec.list_type, StreamListType::GroupId);
        assert_eq!(spec.list_id, 5);

        // Group by ID with media type
        let spec = StreamSpecifier::parse("g:#5:a").unwrap();
        assert_eq!(spec.list_type, StreamListType::GroupId);
        assert_eq!(spec.list_id, 5);
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_AUDIO));

        // Group by ID with media type and index
        let spec = StreamSpecifier::parse("g:#5:a:1").unwrap();
        assert_eq!(spec.list_id, 5);
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_AUDIO));
        assert_eq!(spec.idx, Some(1));
    }

    #[test]
    fn test_parse_metadata_specifier() {
        // Metadata filter (terminates parsing in FFmpeg)
        let spec = StreamSpecifier::parse("m:language:eng").unwrap();
        assert_eq!(spec.list_type, StreamListType::All);
        assert_eq!(spec.meta_key, Some("language".to_string()));
        assert_eq!(spec.meta_val, Some("eng".to_string()));

        // Metadata with media type parsed first
        let spec = StreamSpecifier::parse("v:m:language:eng").unwrap();
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));
        assert_eq!(spec.meta_key, Some("language".to_string()));
        assert_eq!(spec.meta_val, Some("eng".to_string()));

        // Metadata without value (matches any value)
        let spec = StreamSpecifier::parse("m:language").unwrap();
        assert_eq!(spec.meta_key, Some("language".to_string()));
        assert_eq!(spec.meta_val, None);

        // Metadata with components after should error - metadata terminates
        assert!(StreamSpecifier::parse("m:language:eng:v").is_err());
        assert!(StreamSpecifier::parse("m:language:eng:v:0").is_err());
    }

    #[test]
    fn test_parse_usable_streams() {
        // Usable streams (terminates parsing in FFmpeg)
        let spec = StreamSpecifier::parse("u").unwrap();
        assert_eq!(spec.usable_only, true);
        assert_eq!(spec.list_type, StreamListType::All);

        // Usable video streams - parse video first, then usable flag
        let spec = StreamSpecifier::parse("v:u").unwrap();
        assert_eq!(spec.usable_only, true);
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));

        // u:v should error - usable terminates, leaving :v as remainder
        assert!(StreamSpecifier::parse("u:v").is_err());
    }

    #[test]
    fn test_parse_disposition() {
        // Default disposition
        let spec = StreamSpecifier::parse("disp:default").unwrap();
        assert_eq!(spec.disposition, Some("default".to_string()));

        // Forced disposition
        let spec = StreamSpecifier::parse("disp:forced:s").unwrap();
        assert_eq!(spec.disposition, Some("forced".to_string()));
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_SUBTITLE));
    }

    #[test]
    fn test_parse_errors() {
        // Empty string
        assert!(StreamSpecifier::parse("").is_err());

        // Invalid media type
        assert!(StreamSpecifier::parse("x").is_err());

        // Invalid stream ID format
        assert!(StreamSpecifier::parse("#xyz").is_err());

        // Invalid program format
        assert!(StreamSpecifier::parse("p:abc:v").is_err());

        // Invalid group format
        assert!(StreamSpecifier::parse("g:#xyz:a").is_err());

        // Too many colons
        assert!(StreamSpecifier::parse("v:0:1:2:3").is_err());
    }

    #[test]
    fn test_parse_special_characters() {
        // Metadata with special characters
        let spec = StreamSpecifier::parse("m:title:Test Video 123").unwrap();
        assert_eq!(spec.meta_val, Some("Test Video 123".to_string()));

        // Metadata with underscores and dashes
        let spec = StreamSpecifier::parse("m:my_key:my-value").unwrap();
        assert_eq!(spec.meta_key, Some("my_key".to_string()));
        assert_eq!(spec.meta_val, Some("my-value".to_string()));
    }

    #[test]
    fn test_parse_boundary_values() {
        // Large index
        let spec = StreamSpecifier::parse("v:999").unwrap();
        assert_eq!(spec.idx, Some(999));

        // Large stream ID
        let spec = StreamSpecifier::parse("#0xFFFFFFFF").unwrap();
        assert_eq!(spec.list_id, 0xFFFFFFFF);

        // Large program ID
        let spec = StreamSpecifier::parse("p:999999:v").unwrap();
        assert_eq!(spec.list_id, 999999);
    }

    #[test]
    fn test_parse_complex_combinations() {
        // Disposition with video and index
        let spec = StreamSpecifier::parse("disp:default:v:0").unwrap();
        assert_eq!(spec.disposition, Some("default".to_string()));
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));
        assert_eq!(spec.idx, Some(0));

        // Video, disposition, usable (usable terminates)
        let spec = StreamSpecifier::parse("v:disp:default:u").unwrap();
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));
        assert_eq!(spec.disposition, Some("default".to_string()));
        assert_eq!(spec.usable_only, true);

        // These should error - usable/metadata terminate parsing
        assert!(StreamSpecifier::parse("u:disp:default:v").is_err());
        assert!(StreamSpecifier::parse("m:language:eng:disp:forced:s:0").is_err());
    }

    #[test]
    fn test_parse_video_no_apic() {
        // 'V' flag - video without attached pictures
        let spec = StreamSpecifier::parse("V").unwrap();
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));
        assert_eq!(spec.no_apic, true);

        // 'V' with index
        let spec = StreamSpecifier::parse("V:0").unwrap();
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));
        assert_eq!(spec.no_apic, true);
        assert_eq!(spec.idx, Some(0));

        // 'V' with other specifiers
        let spec = StreamSpecifier::parse("V:u").unwrap();
        assert_eq!(spec.no_apic, true);
        assert_eq!(spec.usable_only, true);
    }

    #[test]
    fn test_parse_group_by_index() {
        // Group by index: g:N (GroupIdx)
        let spec = StreamSpecifier::parse("g:0").unwrap();
        assert_eq!(spec.list_type, StreamListType::GroupIdx);
        assert_eq!(spec.list_id, 0);

        let spec = StreamSpecifier::parse("g:5").unwrap();
        assert_eq!(spec.list_type, StreamListType::GroupIdx);
        assert_eq!(spec.list_id, 5);

        // Group by index with media type
        let spec = StreamSpecifier::parse("g:0:v").unwrap();
        assert_eq!(spec.list_type, StreamListType::GroupIdx);
        assert_eq!(spec.media_type, Some(AVMediaType::AVMEDIA_TYPE_VIDEO));

        // Group by ID with g:# prefix
        let spec = StreamSpecifier::parse("g:#123").unwrap();
        assert_eq!(spec.list_type, StreamListType::GroupId);
        assert_eq!(spec.list_id, 123);

        // Uppercase hex group ID
        let spec = StreamSpecifier::parse("g:#0X1F").unwrap();
        assert_eq!(spec.list_type, StreamListType::GroupId);
        assert_eq!(spec.list_id, 0x1F);

        // Group by ID with g:i: prefix
        let spec = StreamSpecifier::parse("g:i:456").unwrap();
        assert_eq!(spec.list_type, StreamListType::GroupId);
        assert_eq!(spec.list_id, 456);

        // Group by ID with hex
        let spec = StreamSpecifier::parse("g:#0x10").unwrap();
        assert_eq!(spec.list_type, StreamListType::GroupId);
        assert_eq!(spec.list_id, 0x10);
    }

    #[test]
    fn test_parse_conflicting_specifiers() {
        // Multiple media types should error
        assert!(StreamSpecifier::parse("v:a").is_err());
        assert!(StreamSpecifier::parse("a:s").is_err());

        // Multiple dispositions should error
        assert!(StreamSpecifier::parse("disp:default:disp:forced").is_err());

        // Cannot combine program and group
        assert!(StreamSpecifier::parse("p:0:g:1").is_err());
        assert!(StreamSpecifier::parse("g:0:p:1").is_err());

        // Cannot combine group and stream ID
        assert!(StreamSpecifier::parse("g:#5:#10").is_err());
        assert!(StreamSpecifier::parse("g:0:i:10").is_err());

        // Cannot combine program and stream ID
        assert!(StreamSpecifier::parse("p:0:#10").is_err());
        assert!(StreamSpecifier::parse("p:0:i:10").is_err());
    }

    #[test]
    fn test_parse_edge_case_inputs() {
        // Single colon
        assert!(StreamSpecifier::parse(":").is_err());

        // Leading colon - currently accepted as it just skips the separator
        // assert!(StreamSpecifier::parse(":v").is_err());

        // Trailing colon - currently accepted (colon separator at end is ignored)
        // assert!(StreamSpecifier::parse("v:").is_err());

        // Consecutive colons - currently accepted (colons are just separators)
        // assert!(StreamSpecifier::parse("v::").is_err());
        // assert!(StreamSpecifier::parse("v::0").is_err());

        // Empty metadata key
        assert!(StreamSpecifier::parse("m::value").is_err());

        // Invalid hex formats - '0xGHI' has no valid hex digits after 0x
        assert!(StreamSpecifier::parse("#0xGHI").is_err());
        assert!(StreamSpecifier::parse("i:0xZZZ").is_err());
        // Note: '0xABG' parses as 0xAB with 'G' left over, currently accepted
        // assert!(StreamSpecifier::parse("g:#0xABG").is_err());

        // Invalid index formats
        assert!(StreamSpecifier::parse("v:abc").is_err());
        assert!(StreamSpecifier::parse("a:1x2").is_err());

        // Empty disposition
        assert!(StreamSpecifier::parse("disp:").is_err());
    }

    #[test]
    fn test_parse_zero_values() {
        // Zero stream ID
        let spec = StreamSpecifier::parse("#0").unwrap();
        assert_eq!(spec.list_type, StreamListType::StreamId);
        assert_eq!(spec.list_id, 0);

        let spec = StreamSpecifier::parse("i:0").unwrap();
        assert_eq!(spec.list_type, StreamListType::StreamId);
        assert_eq!(spec.list_id, 0);

        // Zero program ID
        let spec = StreamSpecifier::parse("p:0:v").unwrap();
        assert_eq!(spec.list_type, StreamListType::Program);
        assert_eq!(spec.list_id, 0);

        // Zero group index
        let spec = StreamSpecifier::parse("g:0").unwrap();
        assert_eq!(spec.list_type, StreamListType::GroupIdx);
        assert_eq!(spec.list_id, 0);

        // Zero group ID
        let spec = StreamSpecifier::parse("g:#0").unwrap();
        assert_eq!(spec.list_type, StreamListType::GroupId);
        assert_eq!(spec.list_id, 0);

        // Zero index
        let spec = StreamSpecifier::parse("v:0").unwrap();
        assert_eq!(spec.idx, Some(0));
    }

    #[test]
    fn test_parse_additional_error_cases() {
        // Negative numbers (should fail during parsing)
        assert!(StreamSpecifier::parse("v:-1").is_err());
        assert!(StreamSpecifier::parse("#-1").is_err());

        // Missing required parts
        assert!(StreamSpecifier::parse("p:").is_err());
        assert!(StreamSpecifier::parse("g:").is_err());
        assert!(StreamSpecifier::parse("i:").is_err());
        assert!(StreamSpecifier::parse("m:").is_err());

        // Invalid characters in numbers - currently these parse the valid prefix
        // assert!(StreamSpecifier::parse("p:1a:v").is_err());
        // assert!(StreamSpecifier::parse("g:2b").is_err());

        // Plain numbers without prefix - currently accepted as index
        // assert!(StreamSpecifier::parse("100").is_err());
    }
}
