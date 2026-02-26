use super::{Body, Endpoint};
use http::Method;
use super::models::*;

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
