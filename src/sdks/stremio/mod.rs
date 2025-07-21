Got it. Here’s the cleaned-up version with:
	•	#[skip_serializing_none] from serde_with
	•	#[derive(Builder)] from bon
	•	No comments or extra annotations

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use bon::Builder;
use std::collections::HashMap;
use crate::client::{Endpoint, QueryParams};

#[derive(Debug, Clone)]
pub struct ManifestEndpoint;

impl Endpoint for ManifestEndpoint {
    type Output = Manifest;

    fn endpoint(&self) -> String {
        "/manifest.json".into()
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub resources: Vec<Resource>,
    pub types: Vec<String>,
    pub catalogs: Vec<CatalogRef>,
    pub id_prefixes: Option<Vec<String>>,
    pub logo: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Resource {
    Simple(String),
    Detailed(ResourceRef),
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ResourceRef {
    pub name: String,
    pub types: Vec<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct CatalogRef {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub extra: Option<Vec<ExtraProp>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ExtraProp {
    pub name: String,
    #[serde(rename = "isRequired")]
    pub is_required: bool,
    pub options: Option<Vec<String>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Builder)]
pub struct CatalogEndpoint {
    pub kind: String,
    pub id: String,
    pub extra: Option<HashMap<String, String>>,
}

impl Endpoint for CatalogEndpoint {
    type Output = CatalogResponse;

    fn endpoint(&self) -> String {
        format!("/catalog/{}/{}.json", self.kind, self.id)
    }

    fn parameters(&self) -> QueryParams {
        self.extra
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect()
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct CatalogResponse {
    pub metas: Vec<MetaItem>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Builder)]
pub struct MetaEndpoint {
    pub kind: String,
    pub id: String,
}

impl Endpoint for MetaEndpoint {
    type Output = MetaResponse;

    fn endpoint(&self) -> String {
        format!("/meta/{}/{}.json", self.kind, self.id)
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct MetaResponse {
    pub meta: MetaItem,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Builder)]
pub struct StreamEndpoint {
    pub kind: String,
    pub id: String,
}

impl Endpoint for StreamEndpoint {
    type Output = StreamResponse;

    fn endpoint(&self) -> String {
        format!("/stream/{}/{}.json", self.kind, self.id)
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct StreamResponse {
    pub streams: Vec<Stream>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct Stream {
    pub title: Option<String>,
    pub url: String,
    pub external_url: Option<String>,
    #[serde(rename = "behaviorHints")]
    pub behavior_hints: Option<HashMap<String, serde_json::Value>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct MetaItem {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub poster: Option<String>,
    pub description: Option<String>,
    pub genres: Option<Vec<String>>,
    pub background: Option<String>,
    pub logo: Option<String>,
}