use crate::{
    CachedEndpoint, ClientError, Endpoint, RestClient,
    remux::{MediaSegments, Segment},
};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const BASE_URL: &str = "https://api.introdb.app";
const CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntroDbSegment {
    start_sec: Option<f64>,
    end_sec: Option<f64>,
}

/// `/segments` returns every known segment type for an episode; any of them may be null.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntroDbResponse {
    intro: Option<IntroDbSegment>,
    recap: Option<IntroDbSegment>,
    outro: Option<IntroDbSegment>,
}

fn to_segment(s: &Option<IntroDbSegment>) -> Option<Segment> {
    match s {
        Some(IntroDbSegment { start_sec: Some(start), end_sec: Some(end) }) if end > start => Some(Segment {
            start_ticks: secs_to_ticks(*start),
            end_ticks: secs_to_ticks(*end),
        }),
        _ => None,
    }
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
            "/segments?imdb_id={}&season={}&episode={}",
            self.imdb_id, self.season, self.episode
        )
    }
}

pub async fn fetch_episode_segments(
    imdb_id: &str,
    season: i64,
    episode: i64,
) -> Result<MediaSegments> {
    let client = RestClient::new(BASE_URL)
        .map_err(|e| anyhow!("introdb client error: {e}"))?
        .with_retry(crate::ExponentialBackoff::builder().build_with_max_retries(3));

    let ep = EpisodeEndpoint {
        imdb_id: imdb_id.to_string(),
        season,
        episode,
    };
    let resp = match client
        .execute(ep.with_cache(CACHE_TTL))
        .await
    {
        Ok(r) => r,
        Err(ClientError::Http { status: 404, .. }) => {
            return Ok(MediaSegments::default());
        }
        Err(e) => return Err(anyhow!("introdb request failed: {e}")),
    };

    Ok(MediaSegments {
        intro: to_segment(&resp.intro),
        recap: to_segment(&resp.recap),
        outro: to_segment(&resp.outro),
        ..MediaSegments::default()
    })
}
