use crate::{api, db, sdks};
use anyhow::Result;
use async_trait::async_trait;
use sqlx::SqlitePool;
use std::str::FromStr;
use uuid::Uuid;

mod aio;
pub use aio::AioSubtitleProvider;

#[async_trait]
pub trait SubtitleProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, media: &db::Media) -> bool;
    async fn fetch(
        &self,
        media: &db::Media,
        db: &SqlitePool,
    ) -> Result<Vec<sdks::aio::Subtitle>>;
}

/// Fetch subtitles for `media` from the first matching provider.
/// Subsequent calls are cheap because providers use the global HTTP cache.
pub async fn fetch(media: &db::Media, db: &SqlitePool) -> Vec<sdks::aio::Subtitle> {
    let providers: &[&dyn SubtitleProvider] = &[&AioSubtitleProvider];
    let mut subs = vec![];
    for p in providers {
        if !p.supports(media) {
            continue;
        }
        //  tracing::debug!(provider = p.name(), item = %media.id, "fetching subtitles");
        match p.fetch(media, db).await {
            Ok(s) => {
                tracing::debug!(provider = p.name(), item = %media.id, count = s.len(), "subtitles fetched");
                subs.extend(s);
            }
            Err(e) => {
                tracing::error!(provider = p.name(), item = %media.id, error = %e, "subtitle provider failed");
            }
        }
    }
    tracing::info!(item = %media.id, count = subs.len(), "subtitles fetched");
    subs
}

/// Inject external subtitles into `media_sources` for playback.
/// Calls `fetch` (HTTP-cached), filters by language preference, scores against
/// each source by filename token overlap, and appends matched streams.
pub async fn inject_into_sources(
    media: &db::Media,
    db: &SqlitePool,
    media_sources: &mut Vec<api::MediaSourceInfo>,
    item_id: Uuid,
    api_key: &str,
) {
    let subs = fetch(media, db).await;
    if subs.is_empty() {
        return;
    }

    let sub_langs: Vec<String> = crate::db::Settings::get_config(db)
        .await
        .ok()
        .and_then(|c| c.subtitle_languages)
        .unwrap_or_default();

    let filtered: Vec<_> = if sub_langs.is_empty() {
        subs
    } else {
        subs.into_iter()
            .filter(|s| {
                let two = s.lang.as_deref().and_then(lang_to_two_letter);
                two.map_or(false, |two| {
                    sub_langs.iter().any(|p| two.eq_ignore_ascii_case(p.trim()))
                })
            })
            .collect()
    };

    if filtered.is_empty() {
        return;
    }

    for source in media_sources.iter_mut() {
        let next_idx = source
            .media_streams
            .iter()
            .map(|s| s.index)
            .max()
            .map_or(0, |m| m + 1);

        let mut scored: Vec<_> = filtered
            .iter()
            .map(|s| (score_sub_url(&s.url, &source.name, &source.path), s))
            .collect();
        scored.sort_by(|(sa, a), (sb, b)| {
            let rank = |s: &&sdks::aio::Subtitle| {
                let two = s.lang.as_deref().and_then(lang_to_two_letter);
                sub_langs
                    .iter()
                    .position(|p| {
                        two.as_deref()
                            .map_or(false, |t| t.eq_ignore_ascii_case(p.trim()))
                    })
                    .unwrap_or(usize::MAX)
            };
            rank(a).cmp(&rank(b)).then(sb.cmp(sa))
        });

        let mut lang_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let scored: Vec<_> = scored
            .into_iter()
            .filter(|(_, s)| {
                let key = s.lang.clone().unwrap_or_else(|| "und".to_string());
                let count = lang_counts.entry(key).or_insert(0);
                if *count < 2 {
                    *count += 1;
                    true
                } else {
                    false
                }
            })
            .collect();

        let wants_default =
            !sub_langs.is_empty() && source.default_subtitle_stream_index.is_none();
        for (i, (_, sub)) in scored.iter().enumerate() {
            let mut stream =
                crate::conversions::subtitle_to_media_stream((*sub).clone());
            let idx = next_idx + i as i64;
            stream.index = idx;
            let encoded_url = urlencoding::encode(&sub.url);
            stream.delivery_url = Some(format!(
                "/Videos/{item_id}/{source_id}/Subtitles/{idx}/0/Stream.vtt?ApiKey={api_key}&SubtitleUrl={encoded_url}",
                source_id = source.id,
            ));
            if wants_default && i == 0 {
                stream.is_default = Some(true);
                source.default_subtitle_stream_index = Some(next_idx);
            }
            source.media_streams.push(stream);
        }
    }
}

pub fn lang_to_two_letter(lang: &str) -> Option<String> {
    let lang = lang.trim().to_lowercase();
    if lang.is_empty() {
        return None;
    }
    if lang.len() == 2 {
        return Some(lang);
    }
    isolang::Language::from_639_3(&lang)
        .or_else(|| isolang::Language::from_str(&lang).ok())
        .and_then(|l| l.to_639_1())
        .map(|s| s.to_string())
}

fn score_sub_url(
    sub_url: &str,
    source_name: &Option<String>,
    source_path: &Option<String>,
) -> i32 {
    fn tokens(s: &str) -> std::collections::HashSet<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() > 2)
            .map(|t| t.to_lowercase())
            .collect()
    }
    let sub_file = sub_url.rsplit('/').next().unwrap_or(sub_url);
    let sub_tok = tokens(sub_file);
    let mut src_tok = tokens(source_name.as_deref().unwrap_or(""));
    src_tok.extend(tokens(source_path.as_deref().unwrap_or("")));
    sub_tok.intersection(&src_tok).count() as i32
}
