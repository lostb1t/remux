use crate::components;
use crate::hooks;
use crate::media;
use crate::sdks;
use crate::server;
use crate::server::MediaQuery;
use crate::Route;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, info};
use dioxus_router::prelude::*;

#[derive(PartialEq, Props, Clone)]
pub struct SearchViewProps {
    pub query: String,
}

#[component]
pub fn SearchView(props: SearchViewProps) -> Element {
    debug!(%props.query, "SearchView");

    let query = props.query.clone();

    rsx! {
        div { class: "",
            components::MediaList {
                key: "search-{query.clone()}",
                title: Some("Search Results".to_string()),
                orientation: components::Orientation::Vertical,
                query: MediaQuery::builder().limit(100).search_query(query.clone()).build(),
            }
        }
    }
}
