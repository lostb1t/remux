use anyhow::{Result, anyhow};
use axum;
use axum::{
    extract::FromRequest,
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use core::fmt::Debug;
use serde::Serialize;
use serde_json::json;
use serde_with::skip_serializing_none;
use std::{error::Error as StdError, fmt};
use tracing::error;

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
        self.map_err(|e| anyhow!("{:?}", e).into())
    }

    fn anyhow(self) -> Result<T> {
        self.map_err(|e| anyhow!("{:?}", e).into())
    }
}
