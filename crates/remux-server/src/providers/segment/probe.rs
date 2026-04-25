use super::SegmentProvider;
use crate::db;
use anyhow::Result;
use async_trait::async_trait;
use remux_sdks::remux::models::MediaSegments;
use sqlx::SqlitePool;

pub struct ProbeSegmentProvider;

#[async_trait]
impl SegmentProvider for ProbeSegmentProvider {
    fn name(&self) -> &'static str {
        "probe"
    }

    fn supports(&self, media: &db::Media) -> bool {
        matches!(media.kind, db::MediaKind::Episode | db::MediaKind::Movie)
    }

    async fn fetch(&self, media: &db::Media, db: &SqlitePool) -> Result<MediaSegments> {
        let sources = db::Media::get_by_filter(
            db,
            &db::MediaFilter {
                parent_id: Some(media.id),
                kind: Some(vec![db::MediaKind::Source]),
                ..Default::default()
            },
        )
        .await
        .map(|r| r.records)
        .unwrap_or_default();

        let mut merged = MediaSegments::default();
        for source in sources {
            if let Some(probe) = source.probe_data {
                if let Some(segs) = probe.0.segments {
                    merged.merge_from(segs);
                }
            }
        }
        Ok(merged)
    }
}
