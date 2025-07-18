use crate::hooks;
use crate::media;
use crate::sdks;
use crate::server;
use crate::utils;
use crate::Route;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, info};
use dioxus_router::prelude::*;
use rand::Rng;
use std::sync::Arc;
use web_sys::{ScrollBehavior, ScrollLogicalPosition, ScrollToOptions};

use super::FadeInImage;

#[derive(PartialEq, Clone)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

impl Default for Orientation {
    fn default() -> Self {
        Self::Horizontal
    }
}

#[derive(Clone, PartialEq, Props)]
pub struct MediaListProps {
    pub title: Option<String>,
    pub query: server::MediaQuery,
    #[props(default)]
    pub orientation: Orientation,
}

#[component]
pub fn MediaList(props: MediaListProps) -> Element {
    let server = hooks::consume_server().expect("missing server");
    let query = props.query.clone();
    let scroll_size = 5;
    let title = props.title.clone().unwrap_or_else(|| "Unknown".to_string());

    let media_items = {
        let server = server.clone();
        let query = query.clone();
        let title = title.clone();

        utils::use_paginated_resource(10, move |limit, offset| {
            let server = server.clone();
            let mut paged_query = query.clone();
            paged_query.offset = offset as u32;
            debug!(
                "{}: Fetching items with offset: {}",
                title, paged_query.offset
            );
            async move { Ok(crate::server::get_media_cached(server, &paged_query).await?) }
        })
        .suspend()?
    };

    let items = media_items().items.read().clone();
    let mut scroll_to = use_signal(|| 0);
    let list_id = use_memo(|| rand::thread_rng().gen::<u32>().to_string());

    rsx! {
        div { class: "px-0 min-w-full",
            div {
                if let Some(title) = props.title.clone() {
                    div { class: "flex items-center justify-between mb-2",
                        h3 {
                            class: {
                                if props.orientation == Orientation::Horizontal {
                                    "pl-6 text-xl w-full font-bold text-white"
                                } else {
                                    "text-xl w-full font-bold text-white"
                                }
                            },
                            "{title}"
                        }
                        if props.orientation == Orientation::Horizontal {
                            Paginator {
                                has_prev: *scroll_to.read() > 0,
                                has_next: media_items().has_more.read().clone(),
                                on_prev: Some(
                                    EventHandler::new(move |_| {
                                        let current = *scroll_to.read();
                                        let target = current - scroll_size;
                                        debug!("scrolling to prev: {}", target);
                                        crate::utils::scroll_to_index(format!("{}-{}", list_id(), target));
                                        scroll_to.set(target);
                                    }),
                                ),
                                on_next: Some(
                                    EventHandler::new(move |_| {
                                        let current = *scroll_to.read();
                                        let target = current + scroll_size;
                                        debug!("scrolling to next: {}", target);
                                        crate::utils::scroll_to_index(format!("{}-{}", list_id(), target));
                                        scroll_to.set(target);
                                    }),
                                ),
                            }
                        }
                    }
                }
            }

            div {
                class: match props.orientation {
                    Orientation::Horizontal => {
                        "flex pl-6 scroll-pl-6 snap-x gap-3 mb-2 scroll-smooth no-scrollbar w-full min-w-full overflow-x-auto overflow-hidden"
                    }
                    Orientation::Vertical => "flex flex-wrap gap-4 w-full h-full min-w-full",
                },
                style: "scrollbar-width: none; -ms-overflow-style: none;",

                for (idx , i) in items.iter().enumerate() {
                    div { id: "{list_id}-{idx}", class: "snap-start",
                        super::Card {
                            image: server.image_url(&i, media::ImageType::Poster),
                            to: Route::MediaDetailView {
                                media_type: i.media_type.clone(),
                                id: i.id.clone(),
                            },
                            if let Some(progress) = i.progress() {


                                div { class: "w-full h-full absolute inset-0 flex flex-col justify-end",
                                    div { class: "absolute bottom-0 left-0 w-full p-2",
                                        super::ProgressBar { progress }
                                    }
                                }
                            }
                        }
                    }
                }

                div {
                    onvisible: {
                        move |evt| {
                            let data = evt.data();
                            if let Ok(is_intersecting) = data.is_intersecting() {
                                if is_intersecting {
                                    info!("intersecting: {}", is_intersecting);
                                    if !*media_items().is_loading.read() {
                                        media_items().load_next();
                                    }
                                }
                            }
                        }
                    },
                    class: "w-full h-6 -mr-[200px]",
                }
            }
        }
    }
}

#[derive(PartialEq, Props, Clone)]
pub struct PaginatorProps {
    #[props(default)]
    pub has_next: bool,
    #[props(default)]
    pub has_prev: bool,
    #[props(default)]
    pub on_next: Option<EventHandler<MouseEvent>>,
    #[props(default)]
    pub on_prev: Option<EventHandler<MouseEvent>>,
}

#[component]
pub fn Paginator(props: PaginatorProps) -> Element {
    let app = hooks::use_app();
    let is_touch = app.read().is_touch;

    if is_touch {
        return rsx! {};
    }

    rsx! {
        div { class: "inset-y-0 flex justify-end items-center gap-2 pointer-events-none",
            if props.has_prev {
                button {
                    class: "bg-black/70 hover:bg-black/90 text-white p-2 rounded-full m-0 flex items-center justify-center",
                    onclick: move |e| {
                        if let Some(cb) = &props.on_prev {
                            cb.call(e);
                        }
                    },
                    svg {
                        class: "w-4 h-4",
                        fill: "none",
                        stroke: "currentColor",
                        stroke_width: "2",
                        view_box: "0 0 24 24",
                        path { d: "M15 19l-7-7 7-7" }
                    }
                }
            }
            if props.has_next {
                div { class: "flex items-center pointer-events-auto",
                    button {
                        class: "bg-black/70 hover:bg-black/90 text-white p-2 rounded-full m-2",
                        onclick: move |e| {
                            if let Some(cb) = &props.on_next {
                                cb.call(e);
                            }
                        },
                        svg {
                            class: "w-4 h-4",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            view_box: "0 0 24 24",
                            path { d: "M9 5l7 7-7 7" }
                        }
                    }
                }
            }
        }
    }
}
