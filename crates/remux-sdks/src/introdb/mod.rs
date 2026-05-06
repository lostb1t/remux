use crate::remux::{MediaSegments, Segment};
use crate::{CachedEndpoint, ClientError, Endpoint, RestClient};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const BASE_URL: &str = "https://api.introdb.app";
const CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntroDbResponse {
    start_sec: Option<f64>,
    end_sec: Option<f64>,
}

fn secs_to_ticks(secs: f64) -> i64 {
    (secs * 10_000_000.0) as i64
}

#[derive(Clone, Serialize)]
struct EpisodeEndpoint {
    imdb_id: String,
    season: i64,
    episode: i64,
}

impl Endpoint for EpisodeEndpoint {
    type Output = IntroDbResponse;

    fn path(&self) -> String {
        format!(
            "/intro?imdb_id={}&season={}&episode={}",
            self.imdb_id, self.season, self.episode
        )
    }
}

pub async fn fetch_episode_segments(
    imdb_id: &str,
    season: i64,
    episode: i64,
) -> Result<MediaSegments> {
    let client =
        RestClient::new(BASE_URL).map_err(|e| anyhow!("introdb client error: {e}"))?;

    let ep = EpisodeEndpoint {
        imdb_id: imdb_id.to_string(),
        season,
        episode,
    };
    let resp = match client.execute(ep.with_cache(CACHE_TTL)).await {
        Ok(r) => r,
        Err(ClientError::Http { status: 404, .. }) => {
            return Ok(MediaSegments::default());
        }
        Err(e) => return Err(anyhow!("introdb request failed: {e}")),
    };

    let mut segs = MediaSegments::default();

    if let (Some(start), Some(end)) = (resp.start_sec, resp.end_sec) {
        segs.intro = Some(Segment {
            start_ticks: secs_to_ticks(start),
            end_ticks: secs_to_ticks(end),
        });
    }

    Ok(segs)
}
