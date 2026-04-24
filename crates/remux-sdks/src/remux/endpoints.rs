use super::models::*;
use super::{Body, Endpoint};
use http::Method;
use uuid::Uuid;

impl Endpoint for PublicSystemInfo {
    type Output = PublicSystemInfo;

    fn path(&self) -> String {
        "/system/info/public".into()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetSessions {
    pub active_within_seconds: Option<i64>,
}

impl Endpoint for GetSessions {
    type Output = Vec<SessionInfoDto>;

    fn path(&self) -> String {
        "/sessions".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        match self.active_within_seconds {
            Some(s) => vec![("activeWithinSeconds".into(), s.to_string())],
            None => vec![],
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetScheduledTasks {
    pub is_hidden: Option<bool>,
}

impl Endpoint for GetScheduledTasks {
    type Output = Vec<TaskInfo>;

    fn path(&self) -> String {
        "/scheduledtasks".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        match self.is_hidden {
            Some(v) => vec![("isHidden".into(), v.to_string())],
            None => vec![],
        }
    }
}

#[derive(Debug, Clone)]
pub struct GetTask {
    pub task_id: String,
}

impl Endpoint for GetTask {
    type Output = TaskInfo;

    fn path(&self) -> String {
        format!("/scheduledtasks/{}", self.task_id)
    }
}

#[derive(Debug, Clone)]
pub struct GetJellyfinItemsByIds {
    pub ids: Vec<String>,
}

impl Endpoint for GetJellyfinItemsByIds {
    type Output = QueryResult<JellyfinItem>;

    fn path(&self) -> String {
        "/Items".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        vec![
            ("Ids".into(), self.ids.join(",")),
            ("Fields".into(), "ProviderIds".into()),
        ]
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetJellyfinUsers;

impl Endpoint for GetJellyfinUsers {
    type Output = Vec<JellyfinUserDto>;

    fn path(&self) -> String {
        "/Users".into()
    }
}

#[derive(Debug, Clone)]
pub struct GetJellyfinUserItems {
    pub user_id: String,
    pub filter: &'static str,
}

impl Endpoint for GetJellyfinUserItems {
    type Output = QueryResult<JellyfinItem>;

    fn path(&self) -> String {
        format!("/Users/{}/Items", self.user_id)
    }

    fn query(&self) -> Vec<(String, String)> {
        vec![
            ("Recursive".into(), "true".into()),
            ("Fields".into(), "ProviderIds,UserData,SeriesId".into()),
            ("IncludeItemTypes".into(), "Movie,Episode".into()),
            ("Filters".into(), self.filter.into()),
        ]
    }
}

#[derive(Debug, Clone)]
pub struct StartTask {
    pub task_id: String,
}

impl Endpoint for StartTask {
    type Output = ();

    fn path(&self) -> String {
        format!("/scheduledtasks/running/{}", self.task_id)
    }

    fn method(&self) -> Method {
        Method::POST
    }
}

#[derive(Debug, Clone)]
pub struct StopTask {
    pub task_id: String,
}

impl Endpoint for StopTask {
    type Output = ();

    fn path(&self) -> String {
        format!("/scheduledtasks/running/{}", self.task_id)
    }

    fn method(&self) -> Method {
        Method::DELETE
    }
}

#[derive(Debug, Clone)]
pub struct UpdateTaskTriggers {
    pub task_id: String,
    pub triggers: Vec<TaskTriggerInfo>,
}

impl Endpoint for UpdateTaskTriggers {
    type Output = ();
    fn path(&self) -> String {
        format!("/scheduledtasks/{}/triggers", self.task_id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.triggers).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetAioCatalogs;

impl Endpoint for GetAioCatalogs {
    type Output = Vec<AioCatalogInfo>;
    fn path(&self) -> String {
        "/aio/catalogs".into()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetAnfiteatroReleaseStatus;

impl Endpoint for GetAnfiteatroReleaseStatus {
    type Output = AnfiteatroReleaseStatus;

    fn path(&self) -> String {
        "/admin/clients/anfiteatro/release".into()
    }
}

#[derive(Debug, Clone, Default)]
pub struct InstallLatestAnfiteatroRelease;

impl Endpoint for InstallLatestAnfiteatroRelease {
    type Output = AnfiteatroInstallResult;

    fn path(&self) -> String {
        "/admin/clients/anfiteatro/release/install".into()
    }

    fn method(&self) -> Method {
        Method::POST
    }
}

#[derive(Debug, Clone)]
pub struct UpdateCatalogSettings {
    pub aio_id: String,
    pub payload: UpdateCatalogSettingsPayload,
}

impl Endpoint for UpdateCatalogSettings {
    type Output = ();
    fn path(&self) -> String {
        format!("/aio/catalogs/{}", self.aio_id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetItems {
    pub include_item_types: Vec<String>,
    pub recursive: bool,
}

impl Endpoint for GetItems {
    type Output = QueryResult<BaseItemDto>;

    fn path(&self) -> String {
        "/items".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        let mut q = vec![];
        if !self.include_item_types.is_empty() {
            q.push(("IncludeItemTypes".into(), self.include_item_types.join(",")));
        }
        if self.recursive {
            q.push(("Recursive".into(), "true".into()));
        }
        q
    }
}

/// Local DB title-contains search for a specific media kind (Genre, Studio, Person, …).
/// Sends `SearchTerm=local:{query}` so the server skips AIO and queries the DB directly.
#[derive(Debug, Clone)]
pub struct GetLocalSuggestions {
    pub kind: String,
    pub search_term: String,
}

impl Endpoint for GetLocalSuggestions {
    type Output = QueryResult<BaseItemDto>;

    fn path(&self) -> String {
        "/items".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        vec![
            ("IncludeItemTypes".into(), self.kind.clone()),
            ("SearchTerm".into(), format!("local:{}", self.search_term)),
            ("Limit".into(), "25".into()),
        ]
    }
}

/// Fetch distinct tag suggestions from the local DB, optionally filtered.
#[derive(Debug, Clone, Default)]
pub struct GetTagSuggestions {
    pub search_term: String,
}

impl Endpoint for GetTagSuggestions {
    type Output = Vec<String>;

    fn path(&self) -> String {
        "/items/tags".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        if self.search_term.is_empty() {
            vec![]
        } else {
            vec![("SearchTerm".into(), self.search_term.clone())]
        }
    }
}

/// Fetch distinct certification values, optionally filtered.
#[derive(Debug, Clone, Default)]
pub struct GetCertificationSuggestions {
    pub search_term: String,
}

impl Endpoint for GetCertificationSuggestions {
    type Output = Vec<String>;

    fn path(&self) -> String {
        "/items/certifications".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        if self.search_term.is_empty() {
            vec![]
        } else {
            vec![("SearchTerm".into(), self.search_term.clone())]
        }
    }
}

/// Fetch the full ISO 3166-1 country list from the locale endpoint.
/// The client filters by name/alpha2 locally (only ~250 entries).
#[derive(Debug, Clone, Default)]
pub struct GetCountries;

impl Endpoint for GetCountries {
    type Output = Vec<CountryInfo>;

    fn path(&self) -> String {
        "/localization/countries".into()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetParentalRatings;

impl Endpoint for GetParentalRatings {
    type Output = Vec<ParentalRating>;

    fn path(&self) -> String {
        "/localization/parentalratings".into()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetVirtualFolders;

impl Endpoint for GetVirtualFolders {
    type Output = Vec<VirtualFolderInfo>;
    fn path(&self) -> String {
        "/library/virtualfolders".into()
    }
}

#[derive(Debug, Clone)]
pub struct CreateVirtualFolder {
    pub payload: CreateVirtualFolderPayload,
}

impl Endpoint for CreateVirtualFolder {
    type Output = VirtualFolderInfo;
    fn path(&self) -> String {
        "/library/virtualfolders".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct UpdateVirtualFolder {
    pub payload: UpdateVirtualFolderPayload,
}

impl Endpoint for UpdateVirtualFolder {
    type Output = ();
    fn path(&self) -> String {
        "/library/virtualfolders/LibraryOptions".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct DeleteVirtualFolder {
    pub name: String,
}

impl Endpoint for DeleteVirtualFolder {
    type Output = ();
    fn path(&self) -> String {
        "/library/virtualfolders".into()
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
    fn query(&self) -> Vec<(String, String)> {
        vec![("name".into(), self.name.clone())]
    }
}

#[derive(Debug, Clone)]
pub struct PatchItem {
    pub item_id: String,
    pub payload: PatchItemPayload,
}

impl Endpoint for PatchItem {
    type Output = ();
    fn path(&self) -> String {
        format!("/items/{}", self.item_id)
    }
    fn method(&self) -> Method {
        Method::PATCH
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetSystemConfiguration;

impl Endpoint for GetSystemConfiguration {
    type Output = ServerConfiguration;
    fn path(&self) -> String {
        "/system/configuration".into()
    }
}

#[derive(Debug, Clone)]
pub struct UpdateSystemConfiguration {
    pub config: ServerConfiguration,
}

impl Endpoint for UpdateSystemConfiguration {
    type Output = ();
    fn path(&self) -> String {
        "/system/configuration".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.config).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetBrandingConfiguration;

impl Endpoint for GetBrandingConfiguration {
    type Output = BrandingOptions;
    fn path(&self) -> String {
        "/branding/configuration".into()
    }
}

#[derive(Debug, Clone)]
pub struct UpdateBrandingConfiguration {
    pub config: BrandingOptions,
}

impl Endpoint for UpdateBrandingConfiguration {
    type Output = ();
    fn path(&self) -> String {
        "/system/configuration/branding".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.config).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetStartupConfiguration;

impl Endpoint for GetStartupConfiguration {
    type Output = StartupConfiguration;
    fn path(&self) -> String {
        "/startup/configuration".into()
    }
}

#[derive(Debug, Clone)]
pub struct PostStartupConfiguration {
    pub payload: StartupConfiguration,
}

impl Endpoint for PostStartupConfiguration {
    type Output = ();
    fn path(&self) -> String {
        "/startup/configuration".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct PostStartupUser {
    pub payload: StartupUser,
}

impl Endpoint for PostStartupUser {
    type Output = ();
    fn path(&self) -> String {
        "/startup/user".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct PostStartupRemoteAccess;

impl Endpoint for PostStartupRemoteAccess {
    type Output = ();
    fn path(&self) -> String {
        "/startup/remoteaccess".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
}

#[derive(Debug, Clone, Default)]
pub struct PostStartupComplete;

impl Endpoint for PostStartupComplete {
    type Output = ();
    fn path(&self) -> String {
        "/startup/complete".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetItemCounts;

impl Endpoint for GetItemCounts {
    type Output = ItemCounts;
    fn path(&self) -> String {
        "/items/counts".into()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetCurrentUser;

impl Endpoint for GetCurrentUser {
    type Output = UserDto;
    fn path(&self) -> String {
        "/users/me".into()
    }
}

#[derive(Debug, Clone)]
pub struct UpdateUserConfiguration {
    pub user_id: Uuid,
    pub config: UserConfiguration,
}

impl Endpoint for UpdateUserConfiguration {
    type Output = ();
    fn path(&self) -> String {
        format!("/users/{}/configuration", self.user_id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.config).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetUsers;

impl Endpoint for GetUsers {
    type Output = Vec<UserDto>;
    fn path(&self) -> String {
        "/users".into()
    }
}

#[derive(Debug, Clone)]
pub struct CreateUser {
    pub name: String,
    pub password: String,
}

impl Endpoint for CreateUser {
    type Output = UserDto;
    fn path(&self) -> String {
        "/users/new".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::json!({ "Name": self.name, "Password": self.password }))
    }
}

#[derive(Debug, Clone)]
pub struct DeleteUser {
    pub user_id: Uuid,
}

impl Endpoint for DeleteUser {
    type Output = ();
    fn path(&self) -> String {
        format!("/users/{}", self.user_id)
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
}

#[derive(Debug, Clone)]
pub struct UpdateUser {
    pub user_id: Uuid,
    pub dto: UserDto,
}

impl Endpoint for UpdateUser {
    type Output = ();
    fn path(&self) -> String {
        format!("/users/{}", self.user_id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.dto).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct UpdateUserPolicy {
    pub user_id: Uuid,
    pub policy: UserPolicy,
}

impl Endpoint for UpdateUserPolicy {
    type Output = ();
    fn path(&self) -> String {
        format!("/users/{}/policy", self.user_id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.policy).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct AdminSetPassword {
    pub user_id: Uuid,
    pub new_pw: String,
}

impl Endpoint for AdminSetPassword {
    type Output = ();
    fn path(&self) -> String {
        format!("/users/{}/password", self.user_id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::json!({ "NewPw": self.new_pw }))
    }
}

impl Endpoint for AuthenticateUserByName {
    type Output = AuthenticateUserByNameResult;

    fn path(&self) -> String {
        "/users/authenticatebyname".into()
    }

    fn method(&self) -> Method {
        Method::POST
    }

    fn body(&self) -> Body {
        Body::Json(serde_json::json!({
            "Username": self.username,
            "Pw": self.pw,
        }))
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetTunerHosts;

impl Endpoint for GetTunerHosts {
    type Output = Vec<TunerHostInfo>;
    fn path(&self) -> String {
        "/livetv/tunerhosts".into()
    }
}

#[derive(Debug, Clone)]
pub struct AddTunerHost {
    pub info: TunerHostInfo,
}

impl Endpoint for AddTunerHost {
    type Output = TunerHostInfo;
    fn path(&self) -> String {
        "/livetv/tunerhosts".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.info).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct DeleteTunerHost {
    pub id: String,
}

impl Endpoint for DeleteTunerHost {
    type Output = ();
    fn path(&self) -> String {
        "/livetv/tunerhosts".into()
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
    fn query(&self) -> Vec<(String, String)> {
        vec![("id".into(), self.id.clone())]
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetEpgSources;

impl Endpoint for GetEpgSources {
    type Output = Vec<EpgSourceInfo>;
    fn path(&self) -> String {
        "/remux/iptv/epgsources".into()
    }
}

#[derive(Debug, Clone)]
pub struct SaveEpgSource {
    pub info: EpgSourceInfo,
}

impl Endpoint for SaveEpgSource {
    type Output = EpgSourceInfo;
    fn path(&self) -> String {
        "/remux/iptv/epgsources".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.info).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct DeleteEpgSource {
    pub id: String,
}

impl Endpoint for DeleteEpgSource {
    type Output = ();
    fn path(&self) -> String {
        "/remux/iptv/epgsources".into()
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
    fn query(&self) -> Vec<(String, String)> {
        vec![("id".into(), self.id.clone())]
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetIptvChannels {
    pub limit: u32,
    pub offset: u32,
    pub search: String,
}

impl Endpoint for GetIptvChannels {
    type Output = IptvChannelsResult;
    fn path(&self) -> String {
        "/remux/iptv/channels".into()
    }
    fn query(&self) -> Vec<(String, String)> {
        let mut q = vec![
            ("limit".into(), self.limit.to_string()),
            ("offset".into(), self.offset.to_string()),
        ];
        if !self.search.is_empty() {
            q.push(("search".into(), self.search.clone()));
        }
        q
    }
}

#[derive(Debug, Clone)]
pub struct PatchChannel {
    pub id: String,
    pub patch: PatchChannelRequest,
}

impl Endpoint for PatchChannel {
    type Output = ();
    fn path(&self) -> String {
        format!("/remux/iptv/channels/{}", self.id)
    }
    fn method(&self) -> Method {
        Method::PATCH
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.patch).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct BulkChannels {
    pub request: BulkChannelRequest,
}

impl Endpoint for BulkChannels {
    type Output = ();
    fn path(&self) -> String {
        "/remux/iptv/channels/bulk".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.request).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct AuthorizeQuickConnect {
    pub code: String,
}

impl Endpoint for AuthorizeQuickConnect {
    type Output = bool;

    fn path(&self) -> String {
        "/quickconnect/authorize".into()
    }

    fn method(&self) -> Method {
        Method::POST
    }

    fn query(&self) -> Vec<(String, String)> {
        vec![("Code".into(), self.code.clone())]
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetApiKeys;

impl Endpoint for GetApiKeys {
    type Output = QueryResult<AuthenticationInfo>;
    fn path(&self) -> String {
        "/auth/keys".into()
    }
}

#[derive(Debug, Clone)]
pub struct CreateApiKey {
    pub app: String,
}

impl Endpoint for CreateApiKey {
    type Output = AuthenticationInfo;
    fn path(&self) -> String {
        "/auth/keys".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn query(&self) -> Vec<(String, String)> {
        vec![("app".into(), self.app.clone())]
    }
}

#[derive(Debug, Clone)]
pub struct DeleteApiKey {
    pub key: String,
}

impl Endpoint for DeleteApiKey {
    type Output = ();
    fn path(&self) -> String {
        format!("/auth/keys/{}", self.key)
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
}
