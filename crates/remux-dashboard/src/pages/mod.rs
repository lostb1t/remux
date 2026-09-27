pub mod addons;
pub mod api_keys;
pub mod branding;
pub mod collections;
pub mod dashboard;
pub mod devices;
pub mod iptv;
pub mod settings;
pub mod streaming;
pub mod streams;
pub mod users;
pub mod webhooks;

use remux_sdks::remux::SortMediaSourcesMode;

const DEVICE_SORTING_HINT: &str = "Start playback once on each device to save its profile for future sorting. Remux learns 4K support after 10 minutes of actual 4K playback without video transcoding; paused time and skips do not count. Until then, smaller streams are preferred unless the profile explicitly supports 4K.";

fn stream_sorting_description(mode: SortMediaSourcesMode) -> &'static str {
    match mode {
        SortMediaSourcesMode::Disabled => {
            "MediaSources stay in probe/addon order — no capability-based sorting."
        }
        SortMediaSourcesMode::Best => {
            "Among streams suited to the device, Direct Play and Direct Stream count equally, so quality picks the winner between them. A version that needs a re-encode ranks below both. Recommended for most setups."
        }
        SortMediaSourcesMode::Compatibility => {
            "Among streams suited to the device, prefer Direct Play, then Direct Stream, then versions that need transcoding."
        }
        SortMediaSourcesMode::Quality => {
            "Best quality (resolution, HDR, bit depth, audio) always wins, even if it means transcoding."
        }
    }
}

pub use addons::AddonsPage;
pub use api_keys::ApiKeysPage;
pub use branding::BrandingPage;
pub use collections::CollectionsPage;
pub use dashboard::DashboardPage;
pub use devices::DevicesPage;
pub use iptv::IptvPage;
pub use settings::{
    IntroSettingsCard, JellyfinImportCard, PlaybackSettingsCard, RemuxdbSettingsCard,
    SearchSettingsCard, ServerSettingsCard,
};
pub use streaming::StreamingGeneralSettingsPage;
pub use streams::StreamGroupsCard;
pub use users::UsersPage;
pub use webhooks::WebhooksPage;
