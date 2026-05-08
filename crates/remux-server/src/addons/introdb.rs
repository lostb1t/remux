use anyhow::{Result, anyhow};
use async_trait::async_trait;
use remux_sdks::remux::MediaSegments;
use std::sync::Arc;
use uuid::Uuid;

use super::{
    AddonKind, AddonMetadata, AddonPreset, AddonPresetRegistration, MediaKind,
    ResourceType,
};
use crate::{AppContext, db};

pub struct IntroDbPreset;

impl AddonPreset for IntroDbPreset {
    fn id(&self) -> &'static str {
        "introdb"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "introdb".to_string(),
            display_name: "IntroDb".to_string(),
            description:
                "Fetches intro/credits timestamps from the community IntroDb database."
                    .to_string(),
            icon: None,
            supported_resources: vec![ResourceType::Segment],
            supported_types: vec![MediaKind::Episode],
            options: vec![],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        _cfg: &serde_json::Value,
    ) -> Result<Arc<dyn AddonKind>> {
        Ok(Arc::new(IntroDbAddon))
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(IntroDbPreset))
}

pub struct IntroDbAddon;

#[async_trait]
impl AddonKind for IntroDbAddon {
    fn id(&self) -> &'static str {
        "introdb"
    }

    fn segment_supports(&self, media: &db::Media) -> bool {
        matches!(media.kind, db::MediaKind::Episode | db::MediaKind::Stream)
            && media.grandparent_media_id.is_some()
            && media.parent_idx.is_some()
            && media.idx.is_some()
    }

    async fn segment_fetch(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<MediaSegments> {
        let imdb_id = media
            .grandparent_media_id
            .as_deref()
            .ok_or_else(|| anyhow!("no grandparent_media_id"))?;
        remux_sdks::introdb::fetch_episode_segments(
            imdb_id,
            media.parent_idx.unwrap(),
            media.idx.unwrap(),
        )
        .await
    }
}
