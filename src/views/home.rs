use crate::components;
use crate::hooks;
use crate::media;
use crate::sdks;
use crate::sdks::core::ApiError;
use crate::server::MediaQuery;

//use anyhow::anyhow;
use crate::views;
use anyhow;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, info};
//use tokio_with_wasm::alias as tokio;

#[derive(Default, PartialEq, Clone, Debug)]
pub struct HomeFilter {
    pub genre: Signal<Option<media::Genre>>,
    pub media_type: Signal<Option<media::MediaType>>,
}

use rand::Rng;
use tracing_subscriber::field::debug;
fn pseudo_random(len: usize) -> usize {
    let mut rng = rand::thread_rng();
    rng.gen_range(0..=20)
}

pub fn HomeTransitionView() -> Element {
    rsx! {
        super::Loading { Home {} }
    }
}

#[component]
pub fn Home() -> Element {
    let server = hooks::use_server()().unwrap();
    let mut home_filter = hooks::use_home_filter();
    let mut settings = crate::settings::use_settings();

    debug!("Home render: {:?}", home_filter);

    let server_clone = server.clone();
    let catalogs = use_resource(move || {
        let server = server_clone.clone();
        async move { crate::server::get_catalogs_cached(server).await }
    })
    .suspend()?;

    let catalogs = catalogs.read();
    // let server_clone = server.clone();

    match &*catalogs {
        Ok(data) => {
            let merged = settings().add_catalogs(data.clone()).catalogs();
            //debug!(?merged, "yo");
            let media_results = use_resource(use_reactive!(|home_filter| {
                let server = server.clone();
                let home_filter = home_filter.clone();
                let merged = merged.clone();

                async move {
                    let media_types = home_filter
                        .media_type
                        .read()
                        .as_ref()
                        .map(|t| vec![t.clone()])
                        .unwrap_or(vec![media::MediaType::Movie, media::MediaType::Series]);
                    let genres = home_filter.genre.read().as_ref().map(|g| vec![g.clone()]);

                    let futures = merged.iter().filter(|x| x.enabled.effective()).map(|col| {
                        let query = MediaQuery::builder()
                            .limit(15)
                            .maybe_genres(genres.clone())
                            .types(media_types.clone())
                            .for_catalog(col.clone())
                            .build();        

                        let server = server.clone();
                        let col = col.clone();
                        async move {
                            let result = crate::server::get_media_cached(server, &query)
                                .await
                                .unwrap_or_default();
                            if result.is_empty() {
                                None
                            } else {
                                Some((col, query))
                            }
                        }
                    });

                    let results: Vec<Option<(_, _)>> = futures::future::join_all(futures).await;

                    Ok::<_, ApiError>(results.into_iter().flatten().collect::<Vec<_>>())
                }
            }))
            .suspend()?;

            let media_results = media_results.read();
            let media_results: Vec<_> = media_results.as_ref().ok().unwrap().clone();

            if let Some((first_col, first_query)) = media_results.get(0) {
                // debug!("First collection: {:?}", first_query);
                // let list_id = rand::thread_rng().gen::<u32>().to_string();
                rsx! {
                    views::HomeMenu {}

                    // Dont know why, but the single item loop makes it at rerender when query changes.
                    // maybe i could just wrap it in a div ahh. whatever
                    for _ in 0..1 {
                        components::HeroList {
                            key: "{first_query.key()}",
                            query: first_query.clone(),
                        }
                    }
                    div { class: "space-y-6",
                        for (col , query) in media_results.iter().skip(1) {
                            // components::MediaList {
                            components::GenericMediaList {
                                class: "sidebar-offset",
                                key: "{query.key()}",
                                title: Some(col.title.clone()),
                                query: query.clone(),
                                card_variant: col.card_variant.effective(),
                            }
                        }
                    }
                }
            } else {
                rsx! {
                    views::HomeMenu {}
                    div { class: "text-center m-20", "No content available" }
                }
            }
        }
        _ => rsx! {
            div {}
        },
    }
}
