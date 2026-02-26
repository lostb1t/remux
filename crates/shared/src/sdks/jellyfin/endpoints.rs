use super::{BasicAuth, ClientError, Endpoint, RestClient};
use http::Method;

use anyhow::Result;
//use chrono::{DateTime, Utc};
use crate::utils;
use chrono::{DateTime, Duration, Utc};
use serde::Deserializer;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::collections::HashMap;
use std::str::FromStr;
use uuid::Uuid;
use models::*;

#[derive(Debug, Clone)]
pub struct AuthenticateUserByNameEndpoint;

impl Endpoint for AuthenticateUserByNameEndpoint {
    type Output = AuthenticateUserByName;

    fn path(&self) -> String {
        "/users/authenticatebyname".into()
    }
}