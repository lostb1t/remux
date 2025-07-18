use super::super::models;
use crate::clients::core::CommaSeparatedList;
use crate::clients::core::Endpoint;
use crate::clients::core::QueryParams;
use derive_builder::Builder;
use http::HeaderMap;
use http::{header, Method, Request};
use itertools::Itertools;
use std::borrow::Cow;

/// Query a members of a project including parent group memberships.
#[derive(Debug, Builder, Clone)]
#[builder(setter(into))]
// #[builder(setter(strip_option))]
pub struct LibraryMedia {
    #[builder(default)]
    r#type: Option<CommaSeparatedList<u64>>,
    #[builder(default = "50")]
    limit: u32,
    #[builder(default = "0")]
    offset: u32,
    #[builder(default)]
    section: Option<u32>,
    #[builder(default)]
    guid: Option<Vec<String>>,
    // query on id is fast on the all library
    #[builder(default)]
    id: Option<Vec<u32>>,
}

impl LibraryMedia {
    /// Create a builder for the endpoint.
    pub fn builder() -> LibraryMediaBuilder {
        LibraryMediaBuilder::default()
    }
}

impl LibraryMediaBuilder {
    pub fn types(&mut self, types: Vec<models::MediaType>) -> &mut Self {
        // let types_id = iter.map(|x| x.value());
        let list = CommaSeparatedList::from(
            types.iter().map(|x| x.value()).collect::<Vec<u64>>(),
        );
        // dbg!(&list);
        self.r#type = Some(Some(list));
        // self.r#type = Some(CommaSeparatedList::from(types.iter().map(|x| x.value()).collect()));
        // self.r#type.get_or_insert_with(BTreeSet::new).extend(iter);
        self
    }
}

impl Endpoint for LibraryMedia {
    type Output = crate::clients::plex::models::Root;

    fn endpoint(&self) -> String {
        if self.section.is_some() {
            return format!("library/sections/{}/all", self.section.clone().unwrap());
        }
        "library/all".to_string()
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-plex-container-size",
            self.limit.to_string().parse().unwrap(),
        );
        headers.insert(
            "x-plex-container-start",
            self.offset.to_string().parse().unwrap(),
        );
        headers
    }

    fn parameters(&self) -> QueryParams {
        let mut params = QueryParams::default();
        params.push("includeGuids", "1");
        if self.r#type.is_some() {
            params.push("type", self.r#type.clone().unwrap());
        }
        if self.guid.is_some() {
            params.push("guid", self.guid.clone().unwrap().iter().join(","));
        }
        if self.id.is_some() {
            params.push("id", self.id.clone().unwrap().iter().join(","));
        }
        params
    }
}
