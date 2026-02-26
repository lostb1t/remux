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
