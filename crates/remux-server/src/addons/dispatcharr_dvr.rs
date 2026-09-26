//! Dispatcharr DVR HTTP client: recordings and series rules.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DispatcharrRecording {
    pub id: i64,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub channel: i64,
    #[serde(default)]
    pub custom_properties: Value,
}

/// `custom_properties` key remux keeps a recording's padding under, so an
/// edit can re-pad from the programme's own times instead of stacking.
pub(crate) const PADDING_KEY: &str = "remux_padding";

/// `custom_properties.remux_padding` for the given padding.
pub(crate) fn padding_properties(pre_seconds: i32, post_seconds: i32) -> Value {
    serde_json::json!({ "pre_seconds": pre_seconds, "post_seconds": post_seconds })
}

/// `recording`'s `custom_properties` with its padding replaced.
pub(crate) fn with_padding(
    recording: &DispatcharrRecording,
    pre_seconds: i32,
    post_seconds: i32,
) -> Value {
    let mut properties = match &recording.custom_properties {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    properties.insert(
        PADDING_KEY.to_string(),
        padding_properties(pre_seconds, post_seconds),
    );
    Value::Object(properties)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispatcharrRecordingStatus {
    /// `custom_properties` carries no `status` key at all for an upcoming,
    /// not-yet-started recording.
    Scheduled,
    Recording,
    Completed,
    Failed,
    Stopped,
    Interrupted,
}

impl DispatcharrRecordingStatus {
    /// Whether the entry is still a timer: not yet started or in progress.
    pub(crate) fn is_timer(self) -> bool {
        matches!(self, Self::Scheduled | Self::Recording)
    }
}

impl DispatcharrRecording {
    pub(crate) fn status(&self) -> DispatcharrRecordingStatus {
        match self
            .custom_properties
            .get("status")
            .and_then(Value::as_str)
        {
            Some("recording") => DispatcharrRecordingStatus::Recording,
            Some("completed") => DispatcharrRecordingStatus::Completed,
            Some("failed") => DispatcharrRecordingStatus::Failed,
            Some("stopped") => DispatcharrRecordingStatus::Stopped,
            Some("interrupted") => DispatcharrRecordingStatus::Interrupted,
            _ => DispatcharrRecordingStatus::Scheduled,
        }
    }

    pub(crate) fn program_title(&self) -> Option<&str> {
        self.custom_properties
            .get("program")
            .and_then(|p| p.get("title"))
            .and_then(Value::as_str)
            .or_else(|| {
                self.custom_properties
                    .get("title")
                    .and_then(Value::as_str)
            })
    }

    pub(crate) fn program_description(&self) -> Option<&str> {
        self.custom_properties
            .get("program")
            .and_then(|p| p.get("description"))
            .and_then(Value::as_str)
            .or_else(|| {
                self.custom_properties
                    .get("description")
                    .and_then(Value::as_str)
            })
    }

    fn program_str(&self, key: &str) -> Option<&str> {
        self.custom_properties
            .get("program")
            .and_then(|p| p.get(key))
            .and_then(Value::as_str)
    }

    /// Dispatcharr's id of the guide programme being recorded
    /// (`dispatcharr::program_key`).
    pub(crate) fn program_key(&self) -> Option<String> {
        self.custom_properties
            .get("program")
            .and_then(|p| p.get("id"))
            .and_then(super::dispatcharr::program_key)
    }

    /// Start and title of the guide programme being recorded.
    pub(crate) fn program_start_and_title(&self) -> Option<(DateTime<Utc>, &str)> {
        Some((
            self.program_window()?
                .0,
            self.program_str("title")?,
        ))
    }

    /// The guide id of the programme being recorded, when Dispatcharr knows
    /// which one it is.
    pub(crate) fn program_tvg_id(&self) -> Option<&str> {
        self.program_str("tvg_id")
    }

    /// The EPG source the recorded programme came from.
    pub(crate) fn program_epg_source_id(&self) -> Option<i64> {
        self.custom_properties
            .get("program")
            .and_then(|p| p.get("epg_source_id"))
            .and_then(Value::as_i64)
    }

    /// The programme's own airing, without any padding either side.
    pub(crate) fn program_window(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let at = |key| {
            self.program_str(key)
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.with_timezone(&Utc))
        };
        Some((at("start_time")?, at("end_time")?))
    }

    /// `(pre, post)` padding in seconds that remux added when it scheduled
    /// or last edited this recording; zero for one it never touched.
    pub(crate) fn padding(&self) -> (i32, i32) {
        let padding = self
            .custom_properties
            .get(PADDING_KEY);
        let secs = |key| {
            padding
                .and_then(|p| p.get(key))
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .unwrap_or(0)
        };
        (secs("pre_seconds"), secs("post_seconds"))
    }

    /// When the recording actually ended. Dispatcharr leaves `end_time` at the
    /// scheduled end of a recording stopped early, and notes the stop as
    /// `stopped_at` and, once the file is finalised, `ended_at`.
    pub(crate) fn actual_end(&self) -> DateTime<Utc> {
        let stopped = (self.status() == DispatcharrRecordingStatus::Stopped)
            .then(|| {
                ["stopped_at", "ended_at"]
                    .into_iter()
                    .find_map(|key| {
                        self.custom_properties
                            .get(key)
                            .and_then(Value::as_str)
                            .and_then(parse_timestamp)
                    })
            })
            .flatten();
        stopped.map_or(self.end_time, |t| t.clamp(self.start_time, self.end_time))
    }

    /// Where Dispatcharr serves the recording: a `/file/` path once finished,
    /// an `/hls/index.m3u8` playlist while still recording.
    pub(crate) fn file_url(&self) -> Option<&str> {
        self.custom_properties
            .get("file_url")
            .or_else(|| {
                self.custom_properties
                    .get("output_file_url")
            })
            .and_then(Value::as_str)
    }
}

/// A timestamp as Dispatcharr stores it, Python's `str()` of a datetime:
/// `2026-09-23 21:52:54.776864+00:00`, or without an offset, in UTC.
fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .or_else(|_| DateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f%:z"))
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|t| t.and_utc())
        })
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub(crate) struct DispatcharrSeriesRule {
    #[serde(default)]
    pub tvg_id: Option<String>,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub epg_source_id: Option<i64>,
    /// `exact` (the default), `contains`, `search` or `regex`.
    #[serde(default)]
    pub title_mode: Option<String>,
    /// Fields remux doesn't read (description mode, pinned channel, …), kept
    /// so re-saving a rule doesn't drop what was set on Dispatcharr's side.
    #[serde(flatten)]
    pub other: serde_json::Map<String, Value>,
}

impl DispatcharrSeriesRule {
    /// Whether this rule would have scheduled `recording`, as far as it can be
    /// told from the programme Dispatcharr stored on it. A rule with a title
    /// is matched only if that title is an exact one: the other title modes
    /// are Dispatcharr's own query syntax. A rule with no title covers any
    /// title. A description filter can't be checked, so such a rule covers
    /// nothing.
    pub(crate) fn covers(&self, recording: &DispatcharrRecording) -> bool {
        if self
            .description
            .as_deref()
            .is_some_and(|d| {
                !d.trim()
                    .is_empty()
            })
        {
            return false;
        }
        let title_matches = match self
            .title
            .as_deref()
        {
            None => true,
            Some(title) => {
                self.title_mode
                    .as_deref()
                    .is_none_or(|m| m.eq_ignore_ascii_case("exact"))
                    && recording
                        .program_title()
                        .is_some_and(|recorded| {
                            title
                                .trim()
                                .to_lowercase()
                                == recorded
                                    .trim()
                                    .to_lowercase()
                        })
            }
        };
        title_matches
            && self
                .tvg_id
                .as_deref()
                .is_none_or(|t| recording.program_tvg_id() == Some(t))
            && self
                .epg_source_id
                .is_none_or(|rule| recording.program_epg_source_id() == Some(rule))
    }

    /// The rule as Dispatcharr's create-or-update endpoint takes it, with
    /// `mode` replaced.
    pub(crate) fn with_mode(&self, mode: &str) -> Value {
        let mut body = self
            .other
            .clone();
        body.insert("tvg_id".into(), serde_json::json!(self.tvg_id));
        body.insert("title".into(), serde_json::json!(self.title));
        body.insert("description".into(), serde_json::json!(self.description));
        body.insert(
            "epg_source_id".into(),
            serde_json::json!(self.epg_source_id),
        );
        if let Some(title_mode) = &self.title_mode {
            body.insert("title_mode".into(), serde_json::json!(title_mode));
        }
        body.insert("mode".into(), serde_json::json!(mode));
        Value::Object(body)
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct SeriesRuleRequest {
    pub tvg_id: Option<String>,
    pub mode: String,
    pub title: Option<String>,
    pub epg_source_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct SeriesRulesResponse {
    rules: Vec<DispatcharrSeriesRule>,
}

pub(crate) async fn schedule_recording(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    channel_id: i64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    custom_properties: &Value,
) -> Result<DispatcharrRecording> {
    let resp = client
        .post(format!("{base_url}/api/channels/recordings/"))
        .header("X-API-Key", token)
        .json(&serde_json::json!({
            "channel": channel_id,
            "start_time": start.to_rfc3339(),
            "end_time": end.to_rfc3339(),
            "custom_properties": custom_properties,
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<DispatcharrRecording>()
        .await?;
    Ok(resp)
}

/// Moves a recording's window. `custom_properties` replaces the stored ones
/// wholesale (Dispatcharr keeps only its own file keys), so pass the current
/// set with any change applied.
pub(crate) async fn update_recording(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    id: i64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    custom_properties: &Value,
) -> Result<DispatcharrRecording> {
    let resp = client
        .patch(format!("{base_url}/api/channels/recordings/{id}/"))
        .header("X-API-Key", token)
        .json(&serde_json::json!({
            "start_time": start.to_rfc3339(),
            "end_time": end.to_rfc3339(),
            "custom_properties": custom_properties,
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<DispatcharrRecording>()
        .await?;
    Ok(resp)
}

pub(crate) async fn list_recordings(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<Vec<DispatcharrRecording>> {
    let resp = client
        .get(format!("{base_url}/api/channels/recordings/"))
        .header("X-API-Key", token)
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<DispatcharrRecording>>()
        .await?;
    Ok(resp)
}

/// `Ok(None)` only for a confirmed 404 — the recording is gone. Any other
/// failure (network, timeout, 5xx, malformed body) is an `Err`, so callers
/// don't mistake a Dispatcharr outage for the recording having vanished.
pub(crate) async fn get_recording(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    id: i64,
) -> Result<Option<DispatcharrRecording>> {
    let resp = client
        .get(format!("{base_url}/api/channels/recordings/{id}/"))
        .header("X-API-Key", token)
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    Ok(Some(
        resp.error_for_status()?
            .json::<DispatcharrRecording>()
            .await?,
    ))
}

pub(crate) async fn delete_recording(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    id: i64,
) -> Result<()> {
    client
        .delete(format!("{base_url}/api/channels/recordings/{id}/"))
        .header("X-API-Key", token)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// Stops a recording that is running, keeping what was recorded so far.
/// Dispatcharr answers `{"success": true, "status": "stopped"}`, not the
/// recording, so the body is not decoded.
pub(crate) async fn stop_recording(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    id: i64,
) -> Result<()> {
    client
        .post(format!("{base_url}/api/channels/recordings/{id}/stop/"))
        .header("X-API-Key", token)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

pub(crate) async fn list_series_rules(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<Vec<DispatcharrSeriesRule>> {
    let resp = client
        .get(format!("{base_url}/api/channels/series-rules/"))
        .header("X-API-Key", token)
        .send()
        .await?
        .error_for_status()?
        .json::<SeriesRulesResponse>()
        .await?;
    Ok(resp.rules)
}

/// Creates the rule, or replaces the one with the same tvg_id, title and
/// source — Dispatcharr's endpoint upserts. It does not schedule anything:
/// follow it with `evaluate_series_rules`.
pub(crate) async fn create_series_rule(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    rule: &impl Serialize,
) -> Result<Vec<DispatcharrSeriesRule>> {
    let resp = client
        .post(format!("{base_url}/api/channels/series-rules/"))
        .header("X-API-Key", token)
        .json(rule)
        .send()
        .await?
        .error_for_status()?
        .json::<SeriesRulesResponse>()
        .await?;
    Ok(resp.rules)
}

/// Schedules whatever the rules for `tvg_id` now match. Dispatcharr's own UI
/// calls this after saving a rule; saving alone schedules nothing until its
/// next EPG refresh.
pub(crate) async fn evaluate_series_rules(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    tvg_id: &str,
) -> Result<()> {
    client
        .post(format!("{base_url}/api/channels/series-rules/evaluate/"))
        .header("X-API-Key", token)
        .json(&serde_json::json!({ "tvg_id": tvg_id }))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

pub(crate) async fn delete_series_rule(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    tvg_id: Option<&str>,
    title: Option<&str>,
    epg_source_id: Option<i64>,
) -> Result<()> {
    let mut params = Vec::new();
    if let Some(v) = tvg_id {
        params.push(format!("tvg_id={}", urlencoding::encode(v)));
    }
    if let Some(v) = title {
        params.push(format!("title={}", urlencoding::encode(v)));
    }
    if let Some(v) = epg_source_id {
        params.push(format!("epg_source_id={v}"));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    client
        .delete(format!("{base_url}/api/channels/series-rules/{qs}"))
        .header("X-API-Key", token)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{Method, MockServer};
    use serde_json::json;

    fn recording(custom_properties: Value) -> DispatcharrRecording {
        serde_json::from_value(json!({
            "id": 5,
            "channel": 42,
            "start_time": "2026-09-15T20:00:00Z",
            "end_time": "2026-09-15T21:00:00Z",
            "custom_properties": custom_properties,
        }))
        .unwrap()
    }

    fn series_rule(v: Value) -> DispatcharrSeriesRule {
        serde_json::from_value(v).unwrap()
    }

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse()
            .unwrap()
    }

    // -- DispatcharrRecording -----------------------------------------

    #[test]
    fn status_maps_every_known_value_and_defaults_to_scheduled() {
        use DispatcharrRecordingStatus as S;
        for (raw, want) in [
            (json!("recording"), S::Recording),
            (json!("completed"), S::Completed),
            (json!("failed"), S::Failed),
            (json!("stopped"), S::Stopped),
            (json!("interrupted"), S::Interrupted),
            (json!("something-new"), S::Scheduled),
            (json!(3), S::Scheduled),
        ] {
            assert_eq!(recording(json!({ "status": raw })).status(), want, "{raw}");
        }
        assert_eq!(recording(Value::Null).status(), S::Scheduled);
        let bare: DispatcharrRecording = serde_json::from_value(json!({
            "id": 1, "channel": 2,
            "start_time": "2026-09-15T20:00:00Z",
            "end_time": "2026-09-15T21:00:00Z",
        }))
        .unwrap();
        assert_eq!(bare.status(), S::Scheduled);
        assert_eq!(bare.program_title(), None);
    }

    #[test]
    fn programme_details_prefer_the_nested_program() {
        let rec = recording(json!({
            "title": "Top level",
            "program": { "title": "Nested", "description": "Nested desc" },
            "description": "Top desc",
        }));
        assert_eq!(rec.program_title(), Some("Nested"));
        assert_eq!(rec.program_description(), Some("Nested desc"));
        let rec =
            recording(json!({ "program": {}, "title": "Top", "description": "Desc" }));
        assert_eq!(rec.program_title(), Some("Top"));
        assert_eq!(rec.program_description(), Some("Desc"));

        let rec = recording(json!({ "program": {
            "start_time": "2026-09-15T20:00:00+00:00",
            "end_time": "2026-09-15T21:00:00+00:00",
        }}));
        assert_eq!(
            rec.program_window(),
            Some((ts("2026-09-15T20:00:00Z"), ts("2026-09-15T21:00:00Z")))
        );
        assert_eq!(recording(json!({})).program_window(), None);
    }

    #[test]
    fn file_url_prefers_file_url_over_output_file_url() {
        let both = recording(json!({ "file_url": "/a", "output_file_url": "/b" }));
        assert_eq!(both.file_url(), Some("/a"));
        let only_output = recording(json!({ "output_file_url": "/b" }));
        assert_eq!(only_output.file_url(), Some("/b"));
        assert_eq!(recording(json!({})).file_url(), None);
    }

    #[test]
    fn padding_round_trips_through_custom_properties() {
        let rec = recording(json!({ "program": { "title": "News" } }));
        assert_eq!(rec.padding(), (0, 0));
        let padded = DispatcharrRecording {
            custom_properties: with_padding(&rec, 120, 300),
            ..rec
        };
        assert_eq!(padded.padding(), (120, 300));
        // The rest of the properties survive.
        assert_eq!(padded.program_title(), Some("News"));
    }

    #[test]
    fn a_stopped_recording_ends_when_it_was_stopped() {
        let end = |props: Value| recording(props).actual_end();
        // As Dispatcharr writes them: `stopped_at` at once, `ended_at` later.
        assert_eq!(
            end(
                json!({ "status": "stopped", "stopped_at": "2026-09-15 20:20:00.776864+00:00" })
            ),
            ts("2026-09-15T20:20:00.776864Z")
        );
        assert_eq!(
            end(json!({
                "status": "stopped",
                "stopped_at": "2026-09-15 20:20:00+00:00",
                "ended_at": "2026-09-15 20:20:21.145489",
            })),
            ts("2026-09-15T20:20:00Z")
        );
        assert_eq!(
            end(
                json!({ "status": "stopped", "ended_at": "2026-09-15 20:20:21.145489" })
            ),
            ts("2026-09-15T20:20:21.145489Z")
        );
        // Only an early stop is trusted, and never past the schedule.
        let scheduled = ts("2026-09-15T21:00:00Z");
        assert_eq!(
            end(json!({ "status": "completed", "ended_at": "2026-09-15 20:20:00" })),
            scheduled
        );
        assert_eq!(
            end(
                json!({ "status": "stopped", "stopped_at": "2026-09-15 22:00:00+00:00" })
            ),
            scheduled
        );
        assert_eq!(
            end(json!({ "status": "stopped", "ended_at": "junk" })),
            scheduled
        );
    }

    // -- series rules -------------------------------------------------

    #[test]
    fn a_series_rule_covers_its_own_airings() {
        let aired = recording(json!({ "program": {
            "title": "Match of the Day", "tvg_id": "BBC1.uk", "epg_source_id": 3,
        }}));
        let covers = |v| series_rule(v).covers(&aired);
        assert!(covers(
            json!({ "tvg_id": "BBC1.uk", "title": "match of the day" })
        ));
        assert!(covers(
            json!({ "tvg_id": "BBC1.uk", "title": "Match of the Day", "epg_source_id": 3 })
        ));
        assert!(covers(json!({ "title": "Match of the Day" })));
        assert!(!covers(
            json!({ "tvg_id": "ITV1.uk", "title": "Match of the Day" })
        ));
        assert!(!covers(
            json!({ "tvg_id": "BBC1.uk", "title": "Match of the Day", "epg_source_id": 4 })
        ));
        assert!(!covers(
            json!({ "tvg_id": "BBC1.uk", "title": "Newsnight" })
        ));
        // Only exact titles can be matched here.
        assert!(!covers(
            json!({ "tvg_id": "BBC1.uk", "title": "Match", "title_mode": "contains" })
        ));
        assert!(!covers(
            json!({ "tvg_id": "BBC1.uk", "description": "football" })
        ));
        // An "All programs" rule has no title to compare, only its channel
        // and source.
        assert!(covers(json!({ "tvg_id": "BBC1.uk", "epg_source_id": 3 })));
        assert!(!covers(json!({ "tvg_id": "ITV1.uk" })));
        assert!(!covers(json!({ "tvg_id": "BBC1.uk", "epg_source_id": 4 })));

        // A source-pinned rule doesn't cover a recording with no source.
        let unsourced = recording(json!({ "program": {
            "title": "Match of the Day", "tvg_id": "BBC1.uk",
        }}));
        assert!(
            !series_rule(
                json!({ "tvg_id": "BBC1.uk", "title": "Match of the Day", "epg_source_id": 3 })
            )
            .covers(&unsourced)
        );
        assert!(
            series_rule(json!({ "tvg_id": "BBC1.uk", "title": "Match of the Day" }))
                .covers(&unsourced)
        );
    }

    #[test]
    fn a_rule_saved_with_a_new_mode_keeps_its_other_fields() {
        let rule = series_rule(json!({
            "tvg_id": "BBC1.uk", "title": "News", "mode": "all", "epg_source_id": 3,
            "title_mode": "exact", "channel_id": 42, "description_mode": "regex",
        }));
        assert_eq!(
            rule.with_mode("new"),
            json!({
                "tvg_id": "BBC1.uk", "title": "News", "mode": "new", "epg_source_id": 3,
                "title_mode": "exact", "channel_id": 42, "description_mode": "regex",
                "description": null,
            })
        );
    }

    // -- HTTP client --------------------------------------------------

    #[tokio::test]
    async fn recording_endpoints_use_the_expected_paths_verbs_and_bodies() {
        let server = MockServer::start();
        let body = json!({
            "id": 9, "channel": 42,
            "start_time": "2026-10-20T03:00:00Z",
            "end_time": "2026-10-20T03:10:00Z",
            "custom_properties": { "status": "recording" },
        });
        let mock = |method: Method, path: &str, sent: Option<Value>, status: u16| {
            server.mock(|when, then| {
                let when = when
                    .method(method)
                    .path(path)
                    .header("X-API-Key", "k");
                if let Some(sent) = sent {
                    when.json_body(sent);
                }
                then.status(status)
                    .json_body(body.clone());
            })
        };
        let list = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/channels/recordings/")
                .header("X-API-Key", "k");
            then.status(200)
                .json_body(json!([body.clone()]));
        });
        let get = mock(Method::GET, "/api/channels/recordings/9/", None, 200);
        let schedule = mock(
            Method::POST,
            "/api/channels/recordings/",
            Some(json!({
                "channel": 42,
                "start_time": "2026-10-20T03:00:00+00:00",
                "end_time": "2026-10-20T03:10:00+00:00",
                "custom_properties": { "title": "News" },
            })),
            201,
        );
        let update = mock(
            Method::PATCH,
            "/api/channels/recordings/9/",
            Some(json!({
                "start_time": "2026-10-20T02:55:00+00:00",
                "end_time": "2026-10-20T03:20:00+00:00",
                "custom_properties": { "remux_padding": { "pre_seconds": 300, "post_seconds": 600 } },
            })),
            200,
        );
        let stop = mock(Method::POST, "/api/channels/recordings/9/stop/", None, 200);
        let del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/channels/recordings/9/")
                .header("X-API-Key", "k");
            then.status(204);
        });
        let evaluate = mock(
            Method::POST,
            "/api/channels/series-rules/evaluate/",
            Some(json!({ "tvg_id": "BBC1.uk" })),
            200,
        );

        let client = reqwest::Client::new();
        let base = server.base_url();
        assert_eq!(
            list_recordings(&client, &base, "k")
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            get_recording(&client, &base, "k", 9)
                .await
                .unwrap()
                .map(|r| r.id),
            Some(9)
        );
        schedule_recording(
            &client,
            &base,
            "k",
            42,
            ts("2026-10-20T03:00:00Z"),
            ts("2026-10-20T03:10:00Z"),
            &json!({ "title": "News" }),
        )
        .await
        .unwrap();
        update_recording(
            &client,
            &base,
            "k",
            9,
            ts("2026-10-20T02:55:00Z"),
            ts("2026-10-20T03:20:00Z"),
            &json!({ "remux_padding": padding_properties(300, 600) }),
        )
        .await
        .unwrap();
        stop_recording(&client, &base, "k", 9)
            .await
            .unwrap();
        delete_recording(&client, &base, "k", 9)
            .await
            .unwrap();
        evaluate_series_rules(&client, &base, "k", "BBC1.uk")
            .await
            .unwrap();

        for m in [&list, &get, &schedule, &update, &stop, &del, &evaluate] {
            m.assert();
        }
    }

    #[tokio::test]
    async fn recording_endpoints_tell_an_outage_from_a_deletion() {
        let server = MockServer::start();
        let mut outage = server.mock(|when, then| {
            when.any_request();
            then.status(500);
        });
        let client = reqwest::Client::new();
        let base = server.base_url();
        assert!(
            get_recording(&client, &base, "k", 1)
                .await
                .is_err()
        );
        assert!(
            stop_recording(&client, &base, "k", 1)
                .await
                .is_err()
        );
        assert!(
            delete_recording(&client, &base, "k", 1)
                .await
                .is_err()
        );
        assert!(
            list_recordings(&client, &base, "k")
                .await
                .is_err()
        );

        // Only a confirmed 404 means the recording is gone.
        outage.delete();
        server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/channels/recordings/1/");
            then.status(404);
        });
        assert!(
            get_recording(&client, &base, "k", 1)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn series_rules_unwrap_the_rules_envelope() {
        let server = MockServer::start();
        let rules = json!({ "rules": [
            { "tvg_id": "BBC1.uk", "mode": "new", "title": "MOTD", "epg_source_id": 3 },
        ]});
        server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/channels/series-rules/")
                .header("X-API-Key", "k");
            then.status(200)
                .json_body(rules.clone());
        });
        let create = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/channels/series-rules/")
                .header("X-API-Key", "k")
                .json_body(json!({
                    "tvg_id": "BBC1.uk", "mode": "new", "title": "MOTD", "epg_source_id": null,
                }));
            then.status(200)
                .json_body(rules.clone());
        });

        let client = reqwest::Client::new();
        let listed = list_series_rules(&client, &server.base_url(), "k")
            .await
            .unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|r| (
                    r.title
                        .as_deref(),
                    r.epg_source_id
                ))
                .collect::<Vec<_>>(),
            [(Some("MOTD"), Some(3))]
        );

        let created = create_series_rule(
            &client,
            &server.base_url(),
            "k",
            &SeriesRuleRequest {
                tvg_id: Some("BBC1.uk".into()),
                mode: "new".into(),
                title: Some("MOTD".into()),
                epg_source_id: None,
            },
        )
        .await
        .unwrap();
        create.assert();
        assert_eq!(created.len(), 1);
    }

    #[tokio::test]
    async fn delete_series_rule_sends_only_the_filters_it_has() {
        let server = MockServer::start();
        let full = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/channels/series-rules/")
                .header("X-API-Key", "k")
                .query_param("tvg_id", "BBC1.uk")
                .query_param("title", "Q&A: 50% off | done")
                .query_param("epg_source_id", "3");
            then.status(204);
        });
        let title_only = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/channels/series-rules/")
                .query_param("title", "MOTD");
            then.status(204);
        });
        let unexpected: Vec<_> = ["tvg_id", "epg_source_id"]
            .map(|param| {
                server.mock(|when, then| {
                    when.query_param("title", "MOTD")
                        .query_param_exists(param);
                    then.status(500);
                })
            })
            .into();
        let client = reqwest::Client::new();
        let base = server.base_url();
        delete_series_rule(
            &client,
            &base,
            "k",
            Some("BBC1.uk"),
            Some("Q&A: 50% off | done"),
            Some(3),
        )
        .await
        .unwrap();
        delete_series_rule(&client, &base, "k", None, Some("MOTD"), None)
            .await
            .unwrap();
        full.assert();
        title_only.assert();
        for m in unexpected {
            m.assert_hits(0);
        }
    }
}
