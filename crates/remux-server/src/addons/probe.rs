use anyhow::Result;
use async_trait::async_trait;
use remux_sdks::remux::models::MediaSegments;

use super::{
    Addon, AddonKind, AddonKindMetadata, AddonKindRegistration, AddonResource, AddonRow,
};
use crate::{AppContext, db};

pub struct ProbeAddonKind;

impl AddonKind for ProbeAddonKind {
    fn id(&self) -> &'static str {
        "probe"
    }

    fn metadata(&self) -> AddonKindMetadata {
        AddonKindMetadata {
            id: "probe".to_string(),
            display_name: "Probe Segments".to_string(),
            description:
                "Extracts chapter/segment markers from the media file's probe data."
                    .to_string(),
            icon: None,
            supported_resources: vec![AddonResource::Segment],
            supported_types: vec!["movie".to_string(), "episode".to_string()],
            options: vec![],
        }
    }

    fn instantiate(&self, row: &AddonRow) -> Result<Box<dyn Addon>> {
        Ok(Box::new(ProbeAddon { row: row.clone() }))
    }
}

inventory::submit! {
    AddonKindRegistration(|| Box::new(ProbeAddonKind))
}

pub struct ProbeAddon {
    row: AddonRow,
}

#[async_trait]
impl Addon for ProbeAddon {
    fn row(&self) -> &AddonRow {
        &self.row
    }

    fn segments_supports(&self, media: &db::Media) -> bool {
        matches!(media.kind, db::MediaKind::Episode | db::MediaKind::Movie)
    }

    async fn segments(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<MediaSegments> {
        let sources = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                parent_id: Some(media.id),
                kind: Some(vec![db::MediaKind::Stream]),
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
