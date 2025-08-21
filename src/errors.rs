use axum;
use axum::{
    extract::FromRequest,
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_valid::Valid as AxumValid;
use core::fmt::Debug;
pub use eyre;
pub use eyre::OptionExt;
use serde::Serialize;
use serde_json::json;
use serde_with::skip_serializing_none;
use tracing::error;
use std::{error::Error as StdError, fmt};

#[derive(Debug, Serialize, Default)]
#[skip_serializing_none]
pub struct ErrorDetail {
    pub error: Option<String>,
    pub description: Option<String>,
    pub errors: Option<serde_json::Value>,
}

/// Error type which implements `IntoResponse`
#[derive(Debug)]
pub struct ApiError(eyre::Report);

/// This enables using `?`
impl From<eyre::Report> for ApiError {
    fn from(error: eyre::Report) -> Self {
        Self(error)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(error: serde_json::Error) -> Self {
        Self(eyre::Report::from(error))
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        Self(eyre::Report::from(error))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        Self(eyre::Report::from(error))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::error!("API Error: {:?}", self.0);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(ErrorDetail {
                error: Some(format!("{:?}", self.0)),
                ..Default::default()
            }),
        )
            .into_response()
    }
}

// Result type

pub type Result<T, E = ApiError> = eyre::Result<T, E>;

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

//#[derive(FromRequest)]
//#[from_request(via(AxumValid), rejection(ApiError))]
//pub struct Valid<T>(pub T);

// We implement `IntoResponse` for our extractor so it can be used as a response
//impl<T: Serialize> IntoResponse for Valid<T> {
//    fn into_response(self) -> axum::response::Response {
//self.0
//      axum::Json(self.0).into_response()
//    }
//}

pub trait LogErr<T, E> {
    fn log_err(self, msg: &str) -> Self;
    fn log_and_unit(self, msg: &str) -> Result<T, ()>;
    fn ok_or_anyhow(self) -> Result<T>;
    fn anyhow(self) -> Result<T>;
}

impl<T, E: Debug> LogErr<T, E> for Result<T, E> {
    fn log_err(self, msg: &str) -> Self {
        if let Err(ref e) = self {
            error!("{}: {:?}", msg, e);
        }
        self
    }

    fn log_and_unit(self, msg: &str) -> Result<T, ()> {
        match self {
            Ok(val) => Ok(val),
            Err(e) => {
                error!("{}: {:?}", msg, e);
                Err(())
            }
        }
    }

    fn ok_or_anyhow(self) -> Result<T> {
        self.map_err(|e| eyre::eyre!("{:?}", e).into())
    }

    fn anyhow(self) -> Result<T> {
        self.map_err(|e| eyre::eyre!("{:?}", e).into())
    }
}
