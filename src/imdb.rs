use anyhow::{Context, Result};
use bytes::Bytes;
use flate2::read::GzDecoder;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use serde_with::{BoolFromInt, DefaultOnError, NoneAsEmptyString, serde_as};

use std::io::{BufRead, BufReader, Cursor, Lines};
use std::pin::Pin;

/// Public struct for one row
#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
// #[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct TitleBasics {
    pub tconst: String,
    pub title_type: String,
    pub primary_title: String,
    pub original_title: String,
    #[serde_as(as = "BoolFromInt")]
    pub is_adult: bool,
    #[serde_as(deserialize_as = "DefaultOnError")]
    pub start_year: Option<u16>,
    #[serde(default, skip_deserializing)]
    pub end_year: Option<u16>,
    #[serde_as(deserialize_as = "DefaultOnError")]
    pub runtime_minutes: Option<u16>,
    #[serde_as(deserialize_as = "DefaultOnError")]
    pub genres: Option<Vec<String>>,
}
