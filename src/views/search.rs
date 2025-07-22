use crate::components;
use crate::hooks;
use crate::media;
use crate::sdks;
use crate::server;
use crate::server::MediaQuery;
use crate::Route;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, info};

#[derive(PartialEq, Props, Clone)]
pub struct SearchViewProps {
    pub query: String,
}

#[component]
pub fn SearchView(props: SearchViewProps) -> Element {
    debug!(%props.query, "SearchView");

    let query = props.query.clone();

    rsx! {
        div {
            div {
                h2 { class: "text-xl font-bold mb-2", "Search Results" }
                components::GenericMediaList {
                    key: "search-{query.clone()}",
                    scroll_direction: components::ScrollDirection::Vertical,
                    query: MediaQuery::builder().search_query(query.clone()).build(),
                }
            
            }
        }
    }
}
