pub mod m3u;
pub mod xmltv;
pub mod xtream;

pub use m3u::{M3uChannel, parse_m3u_stream};
pub use xmltv::{EpgProgram, parse_xmltv};
pub use xtream::{
    fetch_series_list, fetch_vod_streams, fetch_xtream_categories,
    fetch_xtream_channels,
};

use anyhow::Result;
use chrono::Utc;
use futures::TryStreamExt;
use tokio_util::io::{StreamReader, SyncIoBridge};
use uuid::Uuid;

use crate::{AppContext, db, db::ProgramKind};

/// Stream-parse an XMLTV EPG, match each programme to a channel by tvg_id,
/// and upsert in batches of 2000.
pub async fn stream_import_epg(
    client: &reqwest::Client,
    url: &str,
    channels: &[(Uuid, Option<String>)],
    ctx: &AppContext,
) -> Result<usize> {
    if channels.is_empty() {
        return Ok(0);
    }

    let tvg_map: std::collections::HashMap<String, Uuid> = channels
        .iter()
        .filter_map(|(id, tvg)| {
            tvg.as_ref()
                .map(|t| (t.clone(), *id))
        })
        .collect();

    let resp = client
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    let is_gzip = resp
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|v| {
            v.to_str()
                .ok()
        })
        .map(|s| s.contains("gzip"))
        .unwrap_or(false)
        || url
            .to_lowercase()
            .ends_with(".gz");
    let byte_stream = resp
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let async_reader = StreamReader::new(byte_stream);
    let handle = tokio::runtime::Handle::current();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<EpgProgram>(500);

    let parse_handle = tokio::task::spawn_blocking(move || -> Result<()> {
        let sync_reader = SyncIoBridge::new_with_handle(async_reader, handle);
        if is_gzip {
            let gz = flate2::read::GzDecoder::new(sync_reader);
            let buf_reader = std::io::BufReader::with_capacity(256 * 1024, gz);
            parse_xmltv(buf_reader, |prog| {
                tx.blocking_send(prog)
                    .ok();
            })
        } else {
            let buf_reader = std::io::BufReader::with_capacity(256 * 1024, sync_reader);
            parse_xmltv(buf_reader, |prog| {
                tx.blocking_send(prog)
                    .ok();
            })
        }
    });

    // Record time before the first upsert so that any program not re-imported
    // during this run (updated_at will be older) can be pruned at the end.
    let import_start = Utc::now().naive_utc();

    let mut batch: Vec<db::Media> = Vec::with_capacity(500);
    let mut total = 0usize;

    while let Some(prog) = rx
        .recv()
        .await
    {
        let Some(&channel_id) = tvg_map.get(&prog.channel_id) else {
            continue;
        };
        let prog_id = Uuid::new_v5(
            &channel_id,
            format!(
                "{}{}",
                prog.start
                    .map(|d| d.to_string())
                    .unwrap_or_default(),
                prog.title
            )
            .as_bytes(),
        );
        let mut media = db::Media {
            id: prog_id,
            title: prog.title,
            kind: db::MediaKind::TvProgram,
            parent_id: Some(channel_id),
            description: prog.description,
            live_start: prog.start,
            live_end: prog.end,
            program_kind: prog.program_kind,
            ..Default::default()
        };
        if let Some(poster_url) = prog.poster {
            media.set_image(db::ImageKind::Primary, poster_url);
        }
        batch.push(media);
        total += 1;

        if batch.len() >= 500 {
            db::Media::upsert(&ctx.db, &batch).await?;
            batch.clear();
        }
    }

    if !batch.is_empty() {
        db::Media::upsert(&ctx.db, &batch).await?;
        drop(batch);
    }

    parse_handle.await??;

    // Delete programs for these channels that were not re-imported in this run.
    // Programs upserted above have updated_at >= import_start; stale ones do not.
    // A guide that matched nothing (empty or broken response) prunes nothing.
    if total > 0 {
        for chunk in channels.chunks(200) {
            let mut qb = sqlx::QueryBuilder::new(
                "DELETE FROM media WHERE kind = 'tv_program' AND updated_at < ",
            );
            qb.push_bind(import_start);
            qb.push(" AND parent_id IN (");
            let mut sep = qb.separated(", ");
            for (id, _) in chunk {
                sep.push_bind(*id);
            }
            qb.push(")");
            qb.build()
                .execute(&ctx.db)
                .await?;
        }
    }

    sqlx::query(
        "UPDATE media SET program_kind = (
            SELECT c.program_kind FROM media c WHERE c.id = media.parent_id
        )
        WHERE kind = 'tv_program' AND program_kind IS NULL",
    )
    .execute(&ctx.db)
    .await?;

    for chunk in channels.chunks(200) {
        let mut qb = sqlx::QueryBuilder::new(
            "DELETE FROM media WHERE kind = 'tv_program' AND live_end < datetime('now', '-1 day') AND parent_id IN (",
        );
        let mut sep = qb.separated(", ");
        for (id, _) in chunk {
            sep.push_bind(*id);
        }
        qb.push(")");
        qb.build()
            .execute(&ctx.db)
            .await?;
    }

    Ok(total)
}

/// Map a free-text category/group string to a `ProgramKind`.
/// Used for both XMLTV `<category>` tags and M3U/Xtream group-title values.
pub fn parse_program_kind(category: &str) -> Option<ProgramKind> {
    let lower = category.to_lowercase();
    let rules: &[(&[&str], ProgramKind)] = &[
        (
            &[
                "movie", "film", "cinema", "cine", "vod", "pelicul", "filme", "kino",
            ],
            ProgramKind::Movie,
        ),
        (
            &[
                "series",
                "episode",
                "soap",
                "sitcom",
                "show",
                "telenovela",
                "serial",
                "miniseries",
            ],
            ProgramKind::Series,
        ),
        (
            &[
                "news",
                "info",
                "actualit",
                "journalism",
                "documentary",
                "current affairs",
                "noticias",
            ],
            ProgramKind::News,
        ),
        (
            &[
                "children", "kids", "youth", "cartoon", "enfant", "jeunesse", "family",
                "disney", "infantil", "kinder",
            ],
            ProgramKind::Kids,
        ),
        (
            &[
                "sport",
                "basketball",
                "baseball",
                "football",
                "soccer",
                "tennis",
                "cricket",
                "golf",
                "rugby",
                "hockey",
                "racing",
                "boxing",
                "wrestling",
                "fighting",
                "mma",
            ],
            ProgramKind::Sports,
        ),
    ];
    rules
        .iter()
        .find(|(terms, _)| {
            terms
                .iter()
                .any(|t| lower.contains(t))
        })
        .map(|(_, kind)| kind.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guide(title: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><tv><programme start="20990101120000 +0000" stop="20990101130000 +0000" channel="ch1"><title>{title}</title></programme></tv>"#
        )
    }

    async fn programmes(ctx: &AppContext, channel: Uuid) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT title FROM media WHERE kind = 'tv_program' AND parent_id = ?",
        )
        .bind(channel)
        .fetch_all(&ctx.db)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_failed_or_empty_guide_keeps_the_programmes() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let channel = Uuid::new_v4();
        db::Media::upsert(
            &ctx.db,
            &[db::Media {
                id: channel,
                title: "Channel 1".into(),
                kind: db::MediaKind::TvChannel,
                ..Default::default()
            }],
        )
        .await
        .unwrap();
        let provider = httpmock::MockServer::start();
        provider.mock(|when, then| {
            when.path("/first.xml");
            then.status(200)
                .body(guide("First"));
        });
        provider.mock(|when, then| {
            when.path("/error.xml");
            then.status(503)
                .body("<html><body>Service Unavailable</body></html>");
        });
        provider.mock(|when, then| {
            when.path("/empty.xml");
            then.status(200)
                .body("");
        });
        provider.mock(|when, then| {
            when.path("/second.xml");
            then.status(200)
                .body(guide("Second"));
        });
        let client = reqwest::Client::new();
        let channels = vec![(channel, Some("ch1".to_string()))];
        let import = |path: &str| {
            let url = provider.url(path);
            let client = &client;
            let channels = &channels;
            async move { stream_import_epg(client, &url, channels, ctx).await }
        };

        assert_eq!(
            import("/first.xml")
                .await
                .unwrap(),
            1
        );
        assert_eq!(programmes(ctx, channel).await, vec!["First"]);

        // A provider error page must not wipe the guide.
        assert!(
            import("/error.xml")
                .await
                .is_err()
        );
        assert_eq!(programmes(ctx, channel).await, vec!["First"]);

        // Neither must an empty response.
        assert_eq!(
            import("/empty.xml")
                .await
                .unwrap(),
            0
        );
        assert_eq!(programmes(ctx, channel).await, vec!["First"]);

        // A real guide still replaces programmes that are no longer listed.
        assert_eq!(
            import("/second.xml")
                .await
                .unwrap(),
            1
        );
        assert_eq!(programmes(ctx, channel).await, vec!["Second"]);
    }
}
