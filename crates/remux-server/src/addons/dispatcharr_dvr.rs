//! Raw Dispatcharr DVR HTTP client: recordings + series rules.
//!
//! These endpoints return plain arrays (`{"rules": […]}` for series rules),
//! not DRF's `{count,results}` envelope.

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

    /// Authoritative playback pointer, regardless of state: a `/file/` path
    /// once finished, an `/hls/index.m3u8` path while still recording — see
    /// Dispatcharr's own `RecordingViewSet.file` docstring, which redirects
    /// there itself. Prefer this over reconstructing a URL from `id`.
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

#[derive(Debug, Clone, Default, Deserialize)]
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
) -> Result<DispatcharrRecording> {
    let resp = client
        .post(format!("{base_url}/api/channels/recordings/"))
        .header("X-API-Key", token)
        .json(&serde_json::json!({
            "channel": channel_id,
            "start_time": start.to_rfc3339(),
            "end_time": end.to_rfc3339(),
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

pub(crate) async fn get_recording(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    id: i64,
) -> Result<DispatcharrRecording> {
    let resp = client
        .get(format!("{base_url}/api/channels/recordings/{id}/"))
        .header("X-API-Key", token)
        .send()
        .await?
        .error_for_status()?
        .json::<DispatcharrRecording>()
        .await?;
    Ok(resp)
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

pub(crate) async fn create_series_rule(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    rule: &SeriesRuleRequest,
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

pub(crate) async fn delete_series_rule(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    tvg_id: Option<&str>,
    title: Option<&str>,
    epg_source_id: Option<i64>,
) -> Result<()> {
    // reqwest 0.13's `.query()` builder needs the (unenabled) `query`
    // feature — build the query string by hand instead.
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

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse()
            .unwrap()
    }

    // -- DispatcharrRecording -----------------------------------------

    #[test]
    fn status_maps_every_known_value() {
        use DispatcharrRecordingStatus as S;
        for (raw, want) in [
            ("recording", S::Recording),
            ("completed", S::Completed),
            ("failed", S::Failed),
            ("stopped", S::Stopped),
            ("interrupted", S::Interrupted),
        ] {
            assert_eq!(recording(json!({ "status": raw })).status(), want, "{raw}");
        }
    }

    #[test]
    fn status_defaults_to_scheduled() {
        use DispatcharrRecordingStatus::Scheduled;
        assert_eq!(recording(json!({})).status(), Scheduled);
        assert_eq!(
            recording(json!({ "status": "something-new" })).status(),
            Scheduled
        );
        assert_eq!(recording(json!({ "status": 3 })).status(), Scheduled);
        assert_eq!(recording(Value::Null).status(), Scheduled);
    }

    #[test]
    fn missing_custom_properties_deserialises() {
        let rec: DispatcharrRecording = serde_json::from_value(json!({
            "id": 1, "channel": 2,
            "start_time": "2026-09-15T20:00:00Z",
            "end_time": "2026-09-15T21:00:00Z",
        }))
        .unwrap();
        assert_eq!(rec.status(), DispatcharrRecordingStatus::Scheduled);
        assert_eq!(rec.program_title(), None);
        assert_eq!(rec.program_description(), None);
        assert_eq!(rec.file_url(), None);
    }

    #[test]
    fn program_title_prefers_the_nested_program() {
        let rec = recording(json!({
            "title": "Top level",
            "program": { "title": "Nested", "description": "Nested desc" },
            "description": "Top desc",
        }));
        assert_eq!(rec.program_title(), Some("Nested"));
        assert_eq!(rec.program_description(), Some("Nested desc"));
    }

    #[test]
    fn program_title_falls_back_to_top_level_keys() {
        let rec = recording(json!({ "title": "Top level", "description": "Top desc" }));
        assert_eq!(rec.program_title(), Some("Top level"));
        assert_eq!(rec.program_description(), Some("Top desc"));

        // A program object without the key still falls through.
        let rec = recording(json!({ "program": {}, "title": "Top level" }));
        assert_eq!(rec.program_title(), Some("Top level"));
    }

    #[test]
    fn file_url_prefers_file_url_over_output_file_url() {
        let both = recording(json!({ "file_url": "/a", "output_file_url": "/b" }));
        assert_eq!(both.file_url(), Some("/a"));
        let only_output = recording(json!({ "output_file_url": "/b" }));
        assert_eq!(only_output.file_url(), Some("/b"));
        assert_eq!(recording(json!({})).file_url(), None);
    }

    // -- series rules -------------------------------------------------

    #[test]
    fn series_rule_tolerates_missing_fields() {
        let rule: DispatcharrSeriesRule = serde_json::from_value(json!({})).unwrap();
        assert_eq!(rule.tvg_id, None);
        assert_eq!(rule.mode, "");
        assert_eq!(rule.title, None);
        assert_eq!(rule.epg_source_id, None);
    }

    #[test]
    fn series_rule_request_serialises_nulls_explicitly() {
        let body = serde_json::to_value(SeriesRuleRequest {
            tvg_id: Some("BBC1.uk".into()),
            mode: "new".into(),
            title: None,
            epg_source_id: None,
        })
        .unwrap();
        assert_eq!(
            body,
            json!({ "tvg_id": "BBC1.uk", "mode": "new", "title": null, "epg_source_id": null })
        );
    }

    // -- HTTP client --------------------------------------------------

    #[tokio::test]
    async fn schedule_recording_posts_channel_and_rfc3339_window() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/channels/recordings/")
                .header("X-API-Key", "k")
                .json_body(json!({
                    "channel": 42,
                    "start_time": "2026-10-20T03:00:00+00:00",
                    "end_time": "2026-10-20T03:10:00+00:00",
                }));
            then.status(201)
                .json_body(json!({
                    "id": 7, "channel": 42,
                    "start_time": "2026-10-20T03:00:00Z",
                    "end_time": "2026-10-20T03:10:00Z",
                    "custom_properties": {},
                }));
        });
        let rec = schedule_recording(
            &reqwest::Client::new(),
            &server.base_url(),
            "k",
            42,
            ts("2026-10-20T03:00:00Z"),
            ts("2026-10-20T03:10:00Z"),
        )
        .await
        .unwrap();
        mock.assert();
        assert_eq!(rec.id, 7);
    }

    #[tokio::test]
    async fn recording_endpoints_use_the_expected_paths_and_verbs() {
        let server = MockServer::start();
        let body = json!({
            "id": 9, "channel": 1,
            "start_time": "2026-09-15T20:00:00Z",
            "end_time": "2026-09-15T21:00:00Z",
            "custom_properties": { "status": "recording" },
        });
        let list = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/channels/recordings/")
                .header("X-API-Key", "k");
            then.status(200)
                .json_body(json!([body.clone()]));
        });
        let get = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/channels/recordings/9/")
                .header("X-API-Key", "k");
            then.status(200)
                .json_body(body.clone());
        });
        let stop = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/channels/recordings/9/stop/")
                .header("X-API-Key", "k");
            then.status(200)
                .json_body(json!({ "success": true, "status": "stopped" }));
        });
        let del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/channels/recordings/9/")
                .header("X-API-Key", "k");
            then.status(204);
        });

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
                .id,
            9
        );
        stop_recording(&client, &base, "k", 9)
            .await
            .unwrap();
        delete_recording(&client, &base, "k", 9)
            .await
            .unwrap();

        list.assert();
        get.assert();
        stop.assert();
        del.assert();
    }

    #[tokio::test]
    async fn recording_endpoints_surface_http_errors() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.any_request();
            then.status(404);
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
            then.status(200).json_body(rules.clone());
        });

        let client = reqwest::Client::new();
        let listed = list_series_rules(&client, &server.base_url(), "k")
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0]
                .title
                .as_deref(),
            Some("MOTD")
        );
        assert_eq!(listed[0].epg_source_id, Some(3));

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
    async fn delete_series_rule_percent_encodes_the_query() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/channels/series-rules/")
                .header("X-API-Key", "k")
                .query_param("tvg_id", "BBC1.uk")
                .query_param("title", "Q&A: 50% off | done")
                .query_param("epg_source_id", "3");
            then.status(204);
        });
        delete_series_rule(
            &reqwest::Client::new(),
            &server.base_url(),
            "k",
            Some("BBC1.uk"),
            Some("Q&A: 50% off | done"),
            Some(3),
        )
        .await
        .unwrap();
        mock.assert();
    }

    #[tokio::test]
    async fn delete_series_rule_omits_absent_filters() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/channels/series-rules/")
                .query_param("title", "MOTD");
            then.status(204);
        });
        let unexpected = server.mock(|when, then| {
            when.any_request()
                .query_param_exists("tvg_id");
            then.status(500);
        });
        let unexpected_epg = server.mock(|when, then| {
            when.any_request()
                .query_param_exists("epg_source_id");
            then.status(500);
        });
        delete_series_rule(
            &reqwest::Client::new(),
            &server.base_url(),
            "k",
            None,
            Some("MOTD"),
            None,
        )
        .await
        .unwrap();
        mock.assert();
        unexpected.assert_hits(0);
        unexpected_epg.assert_hits(0);
    }
}
