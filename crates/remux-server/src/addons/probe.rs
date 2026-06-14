use anyhow::Result;
use async_trait::async_trait;
use remux_sdks::remux::MediaSegments;
use std::sync::Arc;
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonPreset, AddonPresetRegistration,
    MediaKind, ResourceType, SegmentAddon,
};
use crate::{AppContext, db};

pub struct ProbePreset;

impl AddonPreset for ProbePreset {
    fn id(&self) -> &'static str {
        "probe"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "probe".to_string(),
            display_name: "Probe Segments".to_string(),
            description:
                "Extracts chapter/segment markers from the media file's probe data."
                    .to_string(),
            icon: None,
            supported_resources: vec![ResourceType::Segment],
            supported_types: vec![MediaKind::Movie, MediaKind::Episode],
            options: vec![],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        _cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let addon = Arc::new(ProbeAddon {});
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            segment: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(ProbePreset))
}

pub struct ProbeAddon {}

#[async_trait]
impl AddonKind for ProbeAddon {
    fn id(&self) -> &'static str {
        "probe"
    }
}

#[async_trait]
impl SegmentAddon for ProbeAddon {
    fn supports(&self, media: &db::Media) -> bool {
        matches!(media.kind, db::MediaKind::Episode | db::MediaKind::Movie)
    }

    async fn segment_fetch(
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
                if let Some(segs) = probe.segments {
                    merged.merge_from(segs);
                }
            }
        }
        Ok(merged)
    }
}
