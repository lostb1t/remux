use anyhow::Result;
use serde::Deserialize;

use super::M3uChannel;

/// A single live stream entry from the Xtream Codes player API.
#[derive(Debug, Deserialize)]
pub struct XtreamStream {
    pub num: Option<i64>,
    pub name: Option<String>,
    pub stream_id: Option<serde_json::Value>, // some providers send int, some string
    pub stream_icon: Option<String>,
    pub epg_channel_id: Option<String>,
    pub category_name: Option<String>,
}

/// Fetch live channels from the Xtream Codes player API and return them as
/// `M3uChannel` values so the rest of the import pipeline is reused unchanged.
pub async fn fetch_xtream_channels(
    client: &reqwest::Client,
    server: &str,
    username: &str,
    password: &str,
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
            let stream_url = format!("{}/{}/{}/{}", base, username, password, stream_id);

            Some(M3uChannel {
                tvg_id: s.epg_channel_id.filter(|s| !s.is_empty()),
                name,
                logo: s.stream_icon.filter(|s| !s.is_empty()),
                group: s.category_name.filter(|s| !s.is_empty()),
                channel_number: s.num,
                url: stream_url,
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
