use crate::{Endpoint, NoAuth, RestClient};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingsEndpoint {
    #[serde(skip)]
    pub kitsu_id: i64,
}

impl Endpoint for MappingsEndpoint {
    type Output = MappingsResponse;

    fn path(&self) -> String {
        format!("anime/{}/mappings", self.kitsu_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingsResponse {
    pub data: Vec<MappingEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingEntry {
    pub attributes: MappingAttributes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingAttributes {
    pub external_site: String,
    pub external_id: String,
}

impl MappingsResponse {
    pub fn tvdb_id(&self) -> Option<i64> {
        self.data
            .iter()
            .find(|e| {
                e.attributes
                    .external_site
                    == "thetvdb"
                    || e.attributes
                        .external_site
                        == "thetvdb/series"
            })
            .and_then(|e| {
                e.attributes
                    .external_id
                    .parse()
                    .ok()
            })
    }
}

pub fn client() -> RestClient<NoAuth> {
    RestClient::new("https://kitsu.io/api/edge/").expect("Kitsu base URL is valid")
}
