use crate::sdks::core::params::FormParams;
use crate::sdks::core::CommaSeparatedList;
use crate::sdks::core::Endpoint;
use crate::sdks::core::QueryParams;
use anyhow::Result;
use bon::Builder;
use http::HeaderMap;
use http::{header, Method, Request};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;

#[derive(Debug, Builder, Clone)]
//#[builder(AuthenticateUserByName(into))]
pub struct AuthenticateUserByName {
    password: String,
    username: String,
}

impl Endpoint for AuthenticateUserByName {
    type Output = AuthenticationResult;

    fn method(&self) -> Method {
        Method::POST
    }

    fn endpoint(&self) -> String {
        "Users/AuthenticateByName".to_string()
    }

    fn body(&self) -> Option<HashMap<&str, String>> {
        let mut map = HashMap::new();

        map.insert("Username", self.username.clone());
        map.insert("Pw", self.password.clone());

        Some(map)
    }
}

//#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationResult {
    pub access_token: Option<String>,
    pub server_id: Option<String>,
    //pub session_info: Option<SessionInfoDto>,
    pub user: Option<super::UserDto>,
}
