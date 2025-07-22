use crate::components;
use crate::hooks;
use crate::media;
use crate::server::MediaQuery;
use crate::views;
use crate::Route;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, error, info};
// use dioxus_primitives::select::{
//     Select, SelectGroup, SelectGroupLabel, SelectItemIndicator, SelectList, SelectOption,
//     SelectTrigger,
// };
//use dioxus_router::prelude::*;
use tracing_subscriber::field::debug;

#[derive(PartialEq, Props, Clone)]
pub struct MediaDetailViewProps {
    pub id: String,
    pub media_type: crate::media::MediaType,
}

pub fn MediaDetailViewTransition(props: MediaDetailViewProps) -> Element {
    rsx! {
        super::Loading {
            MediaDetailView { ..props }
        }
    }
}

#[component]
pub fn MediaDetailView(props: MediaDetailViewProps) -> Element {
    debug!(%props.id, ?props.media_type, "MediaDetailView");

    let mut player = components::video::use_video_player();
    let server = hooks::use_server();
    //let mut top_nav = hooks::use_top_nav();
    let navigator = use_navigator();

    //use_effect(move || {
    //    top_nav.set(vec![
    //        views::TopNavItem::Button {
    //            label: "Back".into(),
    //            align: views::NavAlign::Left,
    //            onclick: EventHandler::new(move |_| {
    //                 navigator.go_back();
    //           }),
    //       },
    //       views::TopNavItem::Button {
    //           label: "Favorite".into(),
    //           align: views::NavAlign::Left,
    //           onclick: EventHandler::new(|_| {
    // TODO: Implement favorite a functionality
    //           }),
    //       },
    //   ]);
    //});

    let item = use_resource(use_reactive!(|props| {
        let id = props.id.clone();
        async move {
            let binding = server.read(); // this extends the lifetime
            let Some(server) = binding.as_ref() else {
                return Err(anyhow::anyhow!("No server connected"));
            };
            // return Err(anyhow::anyhow!("No server connected"));
            //Ok(media::Media {id: "hah".to_string(), title: "hah".to_string(), ..Default::default()})
            server.get_media_details(id).await
        }
    }))
    .suspend()?;

    // let item = &*item.read();

    //let item = item.read();
    // debug!(%item, "MediaDetailView");

    let binding = server.read(); // holds the lock
    let server = binding.as_ref().unwrap().clone(); // get Arc<dyn Server>
    rsx! {
        match &*item.read_unchecked() {
            Ok(Some(data)) => rsx! {
                div { class: "sidebar-offset fixed top-6 left-4 flex items-center gap-2 z-10 pt-[env(safe-area-inset-top)] px-[env(safe-area-inset-left)]",
                    button {
                        onclick: move |_| {
                            navigator.go_back();
                        },
                        class: "px-4 py-1.5 rounded border-white/50  bg-black/40 text-sm font-semibold backdrop-blur-sm hover:bg-white/10",
                        svg {
                            xmlns: "http://www.w3.org/2000/svg",
                            fill: "none",
                            view_box: "0 0 24 24",
                            stroke_width: "2",
                            stroke: "currentColor",
                            class: "w-6 h-6", // or adjust size as needed
                            path {
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                                d: "M15 19l-7-7 7-7",
                            }
                        }
                    }
                }
                components::HeroItem { item: data.clone(), detail: true }
                if data.is_series() {
                    // SuspenseBoundary {
                    //     fallback: |context: SuspenseContext| rsx! {
                    //         // Loading {}
                    //         div {}
                    //     },
                    //     MediaDetailSeason { item: data.clone() }
                    // }
                    MediaDetailSeason { item: data.clone() }
                }
            },
            _ => rsx! { "Not found" },
        }
    }
}

#[derive(PartialEq, Props, Clone)]
pub struct MediaDetailSeasonProps {
    pub item: media::Media, // This should be the show itself
}

#[component]
pub fn MediaDetailSeason(props: MediaDetailSeasonProps) -> Element {
    let server = hooks::consume_server().unwrap();
    let item = props.item.clone();

    let seasons = {
        let server = server.clone();
        let item = item.clone();
        use_resource(move || {
            let server = server.clone();
            let item = item.clone();
            async move {
                server
                    .get_media(
                        &MediaQuery::builder()
                            .parent(item.clone())
                            .types(vec![media::MediaType::Season])
                            .build(),
                    )
                    .await
            }
        })
    }
    .suspend()?;

    let seasons = seasons.read().as_ref().unwrap().clone();
    let mut selected_season = use_signal(|| seasons.get(0).cloned());

    let episodes = {
        let server = server.clone();
        use_resource(move || {
            let server = server.clone();
            let selected = selected_season.read().clone();
            async move {
                let Some(season) = selected else {
                    return Ok(vec![]);
                };

                server
                    .get_media(
                        &MediaQuery::builder()
                            .parent(season)
                            .types(vec![media::MediaType::Episode])
                            .build(),
                    )
                    .await
            }
        })
    };
    // .suspend()?;
    // let episodes = if *first_load.read() {
    //     first_load.set(false);
    //     // episodes.suspend()?.read().as_ref().clone()
    //     *episodes.suspend()?.read()
    // } else {
    //     *episodes.read().as_ref().unwrap().clone()
    // };
    rsx! {
        div { class: "sidebar-offset p-4 space-y-4",
            select {
                class: "bg-gray-800 text-white px-3 py-2 rounded",
                onchange: move |evt| {
                    let id = evt.value().clone();
                    let found = seasons.iter().find(|s| s.id == id);
                    if let Some(season) = found.cloned() {
                        selected_season.set(Some(season));
                    }
                },
                for season in seasons.iter() {
                    option {
                        value: "{season.id}",
                        selected: selected_season.read().as_ref().map(|s| s.id == season.id).unwrap_or(false),
                        "{season.title}"
                    }
                }
            }

            match episodes.read().as_ref() {
                Some(Ok(list)) => {
                    rsx! {
                        div { class: "overflow-x-auto no-scrollbar mb-4",
                            div { class: "flex space-x-4",
                                for episode in list.iter() {
                                    // {info!("Rendering episode: {:?}", episode)}
                                    components::Card {
                                        //title: episode.title.clone(),
                                        image: server.image_url(&episode, media::ImageType::Poster).unwrap(),
                                        variant: components::CardVariant::Landscape,
                                        to: Route::MediaDetailView {
                                            media_type: episode.media_type.clone(),
                                            id: episode.id.clone(),
                                        },
                                        class: "w-45",
                                        div { class: "w-full h-full p-5 relative bg-gradient-to-t from-neutral-800 to-neutral-500/10 flex flex-col",
                                            //   div {
                                            //     class: "absolute bottom-0 left-0 bg-gradient-to-t from-neutral-500 to-neutral-200 pointer-events-none"
                                            //   }
                                            p { class: "text-xs text-semibold uppercase",
                                                "Episode {episode.index_number.clone().unwrap()}"
                                            }
                                            p { class: "text-sm text-semibold", "{episode.title.clone()}" }
                                            p { class: "line-clamp-4 text-xs",
                                                "{episode.description.clone().unwrap_or_default()}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                _ => rsx! {},
            }
        }
    }
}
