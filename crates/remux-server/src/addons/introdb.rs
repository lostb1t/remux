use anyhow::{Result, anyhow};
use async_trait::async_trait;
use remux_sdks::remux::models::MediaSegments;

use super::{
    Addon, AddonKind, AddonKindMetadata, AddonKindRegistration, AddonResource, AddonRow,
};
use crate::{AppContext, db};

pub struct IntroDbAddonKind;

impl AddonKind for IntroDbAddonKind {
    fn id(&self) -> &'static str {
        "introdb"
    }

    fn metadata(&self) -> AddonKindMetadata {
        AddonKindMetadata {
            id: "introdb".to_string(),
            display_name: "IntroDb".to_string(),
            description:
                "Fetches intro/credits timestamps from the community IntroDb database."
                    .to_string(),
            icon: None,
            supported_resources: vec![AddonResource::Segment],
            supported_types: vec!["episode".to_string()],
            options: vec![],
        }
    }

    fn instantiate(&self, row: &AddonRow) -> Result<Box<dyn Addon>> {
        Ok(Box::new(IntroDbAddon { row: row.clone() }))
    }
}

inventory::submit! {
    AddonKindRegistration(|| Box::new(IntroDbAddonKind))
}

pub struct IntroDbAddon {
    row: AddonRow,
}

#[async_trait]
impl Addon for IntroDbAddon {
    fn row(&self) -> &AddonRow {
        &self.row
    }

    fn segments_supports(&self, media: &db::Media) -> bool {
        matches!(media.kind, db::MediaKind::Episode | db::MediaKind::Stream)
            && media.series_media_id.is_some()
            && media.parent_idx.is_some()
            && media.idx.is_some()
    }

    async fn segments(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<MediaSegments> {
        let imdb_id = media
            .series_media_id
            .as_deref()
            .ok_or_else(|| anyhow!("no series_media_id"))?;
        remux_sdks::introdb::fetch_episode_segments(
            imdb_id,
            media.parent_idx.unwrap(),
            media.idx.unwrap(),
        )
        .await
    }
}
