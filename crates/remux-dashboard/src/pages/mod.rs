pub mod activity_log;
pub mod addons;
pub mod api_keys;
pub mod appearance;
pub mod branding;
pub mod collections;
pub mod dashboard;
pub mod devices;
pub mod iptv;
pub mod logs;
pub mod settings;
pub mod streams;
pub mod telemetry;
pub mod users;

pub use activity_log::ActivityLogPage;
pub use addons::AddonsPage;
pub use api_keys::ApiKeysPage;
pub use appearance::AppearancePage;
pub use branding::BrandingPage;
pub use collections::CollectionsPage;
pub use dashboard::DashboardPage;
pub use devices::DevicesPage;
pub use iptv::IptvPage;
pub use logs::LogsPage;
pub use settings::{
    IntroSettingsCard, JellyfinImportCard, P2pSettingsCard, PlaybackSettingsCard,
    ProbeSettingsCard, SearchSettingsCard, ServerSettingsCard,
};
pub use streams::StreamGroupsCard;
pub use telemetry::TelemetryPage;
pub use users::UsersPage;
