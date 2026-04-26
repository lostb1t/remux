use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;

use super::M3uChannel;
use crate::db::ProgramKind;

/// A single live stream entry from the Xtream Codes player API.
#[derive(Debug, Deserialize)]
pub struct XtreamStream {
    pub num: Option<i64>,
    pub name: Option<String>,
    pub stream_id: Option<serde_json::Value>, // some providers send int, some string
    pub stream_icon: Option<String>,
    pub epg_channel_id: Option<String>,
    pub category_id: Option<serde_json::Value>, // int or string depending on provider
    pub category_name: Option<String>,
}

/// A category entry from the Xtream Codes `get_live_categories` endpoint.
#[derive(Debug, Deserialize)]
pub struct XtreamCategory {
    pub category_id: Option<serde_json::Value>,
    pub category_name: Option<String>,
}

/// Fetch live categories and return a map of `category_id → ProgramKind`.
/// Returns an empty map on failure (non-fatal).
pub async fn fetch_xtream_categories(
    client: &reqwest::Client,
    server: &str,
    username: &str,
    password: &str,
) -> HashMap<String, ProgramKind> {
    let base = server.trim_end_matches('/');
    let url = format!(
        "{}/player_api.php?username={}&password={}&action=get_live_categories",
        base, username, password
    );
    let Ok(resp) = client.get(&url).send().await else {
        return HashMap::new();
    };
    let Ok(cats) = resp.json::<Vec<XtreamCategory>>().await else {
        return HashMap::new();
    };
    cats.into_iter()
        .filter_map(|c| {
            let id = json_value_to_string(c.category_id.as_ref()?)?;
            let kind = super::parse_program_kind(c.category_name.as_deref()?)?;
            Some((id, kind))
        })
        .collect()
}

/// Fetch live channels from the Xtream Codes player API and return them as
/// `M3uChannel` values so the rest of the import pipeline is reused unchanged.
pub async fn fetch_xtream_channels(
    client: &reqwest::Client,
    server: &str,
    username: &str,
    password: &str,
    category_kinds: &HashMap<String, ProgramKind>,
) -> Result<Vec<M3uChannel>> {
    let base = server.trim_end_matches('/');
    let url = format!(
        "{}/player_api.php?username={}&password={}&action=get_live_streams",
        base, username, password
    );

    let resp = client.get(&url).send().await?;
    let streams: Vec<XtreamStream> = resp.json().await?;

    let channels = streams
        .into_iter()
        .filter_map(|s| {
            let name = s.name.filter(|n| !n.is_empty())?;
            let stream_id = stream_id_to_string(&s.stream_id?)?;
            let stream_url =
                format!("{}/{}/{}/{}", base, username, password, stream_id);

            // Derive program_kind: prefer category_id lookup, fall back to category_name text
            let program_kind = s
                .category_id
                .as_ref()
                .and_then(json_value_to_string)
                .as_deref()
                .and_then(|id| category_kinds.get(id))
                .cloned()
                .or_else(|| {
                    s.category_name
                        .as_deref()
                        .and_then(super::parse_program_kind)
                });

            let group = s.category_name.filter(|s| !s.is_empty());

            Some(M3uChannel {
                tvg_id: s.epg_channel_id.filter(|s| !s.is_empty()),
                name,
                logo: s.stream_icon.filter(|s| !s.is_empty()),
                group,
                channel_number: s.num,
                url: stream_url,
                program_kind,
            })
        })
        .collect();

    Ok(channels)
}

fn stream_id_to_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn json_value_to_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}
