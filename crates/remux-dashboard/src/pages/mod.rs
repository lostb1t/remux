pub mod addons;
pub mod api_keys;
pub mod branding;
pub mod collections;
pub mod dashboard;
pub mod iptv;
pub mod settings;
pub mod streams;
pub mod users;

pub use addons::AddonsPage;
pub use api_keys::ApiKeysPage;
pub use branding::BrandingPage;
pub use collections::CollectionsPage;
pub use dashboard::DashboardPage;
pub use iptv::IptvPage;
pub use settings::{
    IntroSettingsCard, JellyfinImportCard, P2pSettingsCard, PlaybackSettingsCard,
    ProbeSettingsCard, RemuxdbSettingsCard, SearchSettingsCard, ServerSettingsCard,
};
pub use streams::StreamGroupsCard;
pub use users::UsersPage;
