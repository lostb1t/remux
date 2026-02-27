use super::{Body, Endpoint};
use http::Method;
use super::models::*;

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

// ── AIO catalogs ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct GetAioCatalogs;

impl Endpoint for GetAioCatalogs {
    type Output = Vec<AioCatalogInfo>;
    fn path(&self) -> String { "/aio/catalogs".into() }
}

// ── Items ──────────────────────────────────────────────────────────

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

// ── Virtual folder (Collection) endpoints ─────────────────────────

#[derive(Debug, Clone, Default)]
pub struct GetVirtualFolders;

impl Endpoint for GetVirtualFolders {
    type Output = Vec<VirtualFolderInfo>;
    fn path(&self) -> String { "/library/virtualfolders".into() }
}

#[derive(Debug, Clone)]
pub struct CreateVirtualFolder {
    pub payload: CreateVirtualFolderPayload,
}

impl Endpoint for CreateVirtualFolder {
    type Output = VirtualFolderInfo;
    fn path(&self) -> String { "/library/virtualfolders".into() }
    fn method(&self) -> Method { Method::POST }
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
    fn path(&self) -> String { "/library/virtualfolders/LibraryOptions".into() }
    fn method(&self) -> Method { Method::POST }
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
    fn path(&self) -> String { "/library/virtualfolders".into() }
    fn method(&self) -> Method { Method::DELETE }
    fn query(&self) -> Vec<(String, String)> {
        vec![("name".into(), self.name.clone())]
    }
}

// ── System configuration ───────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct GetSystemConfiguration;

impl Endpoint for GetSystemConfiguration {
    type Output = ServerConfiguration;
    fn path(&self) -> String { "/system/configuration".into() }
}

#[derive(Debug, Clone)]
pub struct UpdateSystemConfiguration {
    pub config: ServerConfiguration,
}

impl Endpoint for UpdateSystemConfiguration {
    type Output = ();
    fn path(&self) -> String { "/system/configuration".into() }
    fn method(&self) -> Method { Method::POST }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.config).unwrap_or_default())
    }
}

// ── Startup wizard endpoints ───────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct GetStartupConfiguration;

impl Endpoint for GetStartupConfiguration {
    type Output = StartupConfiguration;
    fn path(&self) -> String { "/startup/configuration".into() }
}

#[derive(Debug, Clone)]
pub struct PostStartupConfiguration {
    pub payload: StartupConfiguration,
}

impl Endpoint for PostStartupConfiguration {
    type Output = ();
    fn path(&self) -> String { "/startup/configuration".into() }
    fn method(&self) -> Method { Method::POST }
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
    fn path(&self) -> String { "/startup/user".into() }
    fn method(&self) -> Method { Method::POST }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct PostStartupRemoteAccess;

impl Endpoint for PostStartupRemoteAccess {
    type Output = ();
    fn path(&self) -> String { "/startup/remoteaccess".into() }
    fn method(&self) -> Method { Method::POST }
}

#[derive(Debug, Clone, Default)]
pub struct PostStartupComplete;

impl Endpoint for PostStartupComplete {
    type Output = ();
    fn path(&self) -> String { "/startup/complete".into() }
    fn method(&self) -> Method { Method::POST }
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
