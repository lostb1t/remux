use std::collections::HashMap;
use std::fmt;

use anyhow::Result;
use futures::TryStreamExt;
use serde::Deserialize;
use serde::de::{Deserializer as _, SeqAccess, Visitor};
use tokio_util::io::{StreamReader, SyncIoBridge};

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
    let Ok(resp) = client
        .get(&url)
        .send()
        .await
    else {
        return HashMap::new();
    };
    let Ok(cats) = resp
        .json::<Vec<XtreamCategory>>()
        .await
    else {
        return HashMap::new();
    };
    cats.into_iter()
        .filter_map(|c| {
            let id = json_value_to_string(
                c.category_id
                    .as_ref()?,
            )?;
            let kind = super::parse_program_kind(
                c.category_name
                    .as_deref()?,
            )?;
            Some((id, kind))
        })
        .collect()
}

/// Fetch live channels from the Xtream Codes player API and return them as
/// `M3uChannel` values so the rest of the import pipeline is reused unchanged.
///
/// Uses a streaming JSON visitor so only one `XtreamStream` exists in memory at
/// a time — the full deserialized array is never materialised.
pub async fn fetch_xtream_channels(
    client: &reqwest::Client,
    server: &str,
    username: &str,
    password: &str,
    category_kinds: &HashMap<String, ProgramKind>,
) -> Result<Vec<M3uChannel>> {
    let base = server
        .trim_end_matches('/')
        .to_owned();
    let url = format!(
        "{}/player_api.php?username={}&password={}&action=get_live_streams",
        base, username, password
    );
    let username = username.to_owned();
    let password = password.to_owned();
    let category_kinds = category_kinds.clone();

    let resp = client
        .get(&url)
        .send()
        .await?;
    let byte_stream = resp
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let async_reader = StreamReader::new(byte_stream);
    let handle = tokio::runtime::Handle::current();

    tokio::task::spawn_blocking(move || -> Result<Vec<M3uChannel>> {
        let sync_reader = SyncIoBridge::new_with_handle(async_reader, handle);
        let buf_reader = std::io::BufReader::with_capacity(256 * 1024, sync_reader);
        let mut de = serde_json::Deserializer::from_reader(buf_reader);

        // Visitor: parses the outer JSON array element by element, converts each
        // XtreamStream to M3uChannel immediately, then drops the stream struct.
        struct StreamCollector {
            base: String,
            username: String,
            password: String,
            category_kinds: HashMap<String, ProgramKind>,
        }

        impl<'de> Visitor<'de> for StreamCollector {
            type Value = Vec<M3uChannel>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "an array of Xtream stream objects")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Vec<M3uChannel>, A::Error> {
                let mut channels = Vec::new();
                while let Some(stream) = seq.next_element::<XtreamStream>()? {
                    if let Some(ch) = stream_to_channel(
                        stream,
                        &self.base,
                        &self.username,
                        &self.password,
                        &self.category_kinds,
                    ) {
                        channels.push(ch);
                    }
                }
                Ok(channels)
            }
        }

        de.deserialize_seq(StreamCollector {
            base,
            username,
            password,
            category_kinds,
        })
        .map_err(anyhow::Error::from)
    })
    .await?
}

fn stream_to_channel(
    s: XtreamStream,
    base: &str,
    username: &str,
    password: &str,
    category_kinds: &HashMap<String, ProgramKind>,
) -> Option<M3uChannel> {
    let name = s
        .name
        .filter(|n| !n.is_empty())?;
    let stream_id = stream_id_to_string(&s.stream_id?)?;
    let stream_url = format!("{}/{}/{}/{}", base, username, password, stream_id);

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

    let group = s
        .category_name
        .filter(|s| !s.is_empty());

    Some(M3uChannel {
        tvg_id: s
            .epg_channel_id
            .filter(|s| !s.is_empty()),
        name,
        logo: s
            .stream_icon
            .filter(|s| !s.is_empty()),
        group,
        channel_number: s.num,
        url: stream_url,
        program_kind,
    })
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
