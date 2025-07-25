use crate::components;
use crate::hooks;
use crate::media;
use crate::sdks;
use crate::server;
use crate::utils;
use crate::Route;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, info};
//use dioxus_router::prelude::*;
use rand::Rng;
use std::rc::Rc;
use std::sync::Arc;
//use web_sys::{ScrollBehavior, ScrollLogicalPosition, ScrollToOptions};

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
    let app = crate::APP_HOST.peek();
    let is_touch = app.is_touch;

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

#[derive(Props, Clone, PartialEq)]
pub struct MediaCardProps {
    pub item: media::Media,
    #[props(default)]
    pub card_variant: components::CardVariant,
}

pub fn MediaCard(props: MediaCardProps) -> Element {
    let server = hooks::use_server()().unwrap();
    let image_type = match &props.card_variant {
        components::CardVariant::Landscape => media::ImageType::Thumb,
        components::CardVariant::Square => media::ImageType::Poster,
        components::CardVariant::Hero => media::ImageType::Poster,
        _ => media::ImageType::Poster,
    };

    let mut title = None;
    let image = if let Some(image) = server.image_url(&props.item, image_type) {
        image
    } else {
        title = Some(props.item.title.clone());
        server
            .image_url(&props.item, media::ImageType::Backdrop)
            .unwrap_or_default()
    };

    rsx! {
        super::Card {
            image,
            variant: props.card_variant,
            to: Route::MediaDetailView {
                media_type: props.item.media_type.clone(),
                id: props.item.id.clone(),
            },
            if let Some(progress) = props.item.progress() {
                div { class: "w-24 p-4 absolute inset-0 justify-end",
                    div { class: "absolute bottom-0 left-0 w-full",
                        super::ProgressBar { progress }
                    }
                }
            }
            if let Some(title) = title {
                div { class: "absolute top-2 left-4 justify-end font-semibold text
              -lg
              ",
                    h4 { "{title}" }
                }
            }
        }
    }
}

// use crate::{media, server, utils, Route};
use dioxus::prelude::*;
// use dioxus_logger::tracing::debug;
// use rand::Rng;

// #[derive(PartialEq, Clone)]
// pub enum Orientation {
//     Horizontal,
//     Vertical,
// }

// impl Default for Orientation {
//     fn default() -> Self {
//         Self::Horizontal
//     }
// }

#[derive(Clone, PartialEq, Props)]
pub struct GenericMediaListProps {
    pub title: Option<String>,
    pub query: server::MediaQuery,
    #[props(default)]
    pub card_variant: components::CardVariant,
    #[props(default)]
    pub scroll_direction: components::ScrollDirection,

    #[props(default)]
    pub class: String,
}

#[component]
pub fn GenericMediaList(props: GenericMediaListProps) -> Element {
    let server = hooks::use_server()().unwrap();
    let scroll_size: usize = 5;
    let title = props.title.clone().unwrap_or_else(|| "Unknown".to_string());
    // let mut scroll_to = use_signal(|| scroll_size);
    let mut scroll_to = use_signal(|| 0);
    let list_id = use_memo(|| rand::thread_rng().gen::<u32>().to_string());

    let query = props.query.clone();
    let media_items = {
        let server = server.clone();
        let query = query.clone();
        let title = title.clone();

        utils::use_paginated_resource(10, move |_, offset| {
            let mut paged_query = query.clone();
            paged_query.offset = offset as u32;
            let server = server.clone();
            debug!("{title}: Fetching offset {}", paged_query.offset);
            async move { Ok(server::get_media_cached(server.clone(), &paged_query).await?) }
        })
        .suspend()?
    };

    let items = media_items().items.read().clone();

    rsx! {
        div { class: "px-0 overflow-x-visible {props.class}",
            if let Some(title) = props.title.clone() {
                div { class: "flex items-center justify-between mb-2",
                    h3 {
                        class: match props.scroll_direction {
                            // components::ScrollDirection::Horizontal => "sidebar-offset  pl-6 text-xl w-full font-bold text-white",
                            components::ScrollDirection::Horizontal => {
                                "pl-6 text-xl w-full font-semibold text-white"
                            }
                            components::ScrollDirection::Vertical => {
                                "text-xl w-full font-semibold text-white"
                            }
                        },
                        "{title}"
                    }

                    if props.scroll_direction == components::ScrollDirection::Horizontal {
                        super::Paginator {
                            // has_prev: *scroll_to.read() > scroll_size,
                            has_next: *media_items().has_more.read(),
                            // on_prev: Some(EventHandler::new({
                            //     debug!("PREV");
                            //     let list_id = list_id.clone();
                            //     move |_| {
                            //         let current = *scroll_to.read() as usize;
                            //         let target = current.saturating_sub(scroll_size);
                            //         debug!(?list_id, ?current, ?target, "on prev");
                            //         //crate::utils::scroll_to_index(format!("{}-{}", list_id(), target));
                            //         scroll_to.set(target);
                            //     }
                            // })),
                            on_next: Some(
                                //crate::utils::scroll_to_index(format!("{}-{}", list_id(), target));
                                EventHandler::new({
                                    let list_id = list_id.clone();
                                    move |_| {
                                        let current = *scroll_to.read();
                                        let mut target = current + scroll_size;
                                        if target <= scroll_size {
                                            target += scroll_size;
                                        }
                                        debug!(? list_id, ? current, ? target, "on next");
                                        scroll_to.set(target);
                                    }
                                }),
                            ),
                        }
                    }
                }
            }

            match props.scroll_direction {
                components::ScrollDirection::Horizontal => rsx! {
                    super::PaginatedList {
                        items: items.clone(),
                        index: scroll_to,
                        class: "overflow-y-hidden overflow-x-auto no-scrollbar pl-6 gap-x-2.5",
                        on_load_more: Some(
                            EventHandler::new(move |_| {
                                if !*media_items().is_loading.read() {
                                    media_items().load_next();
                                }
                            }),
                        ),

        

        
                        render_item: move |i: &media::Media| rsx! {
        

            
                            super::MediaCard { card_variant: props.card_variant.clone(), item: i.clone() }
                        },
                    }
                },
                components::ScrollDirection::Vertical => rsx! {
                    super::PaginatedList {
                        scroll_direction: props.scroll_direction,
                        items: items.clone(),
                        class: "w-full overflow-x-hidden flex flex-wrap",
                        on_load_more: Some(
                            EventHandler::new(move |_| {
                                if !*media_items().is_loading.read() {
                                    media_items().load_next();
                                }
                            }),
                        ),
                        render_item: move |i: &media::Media| rsx! {
                            super::MediaCard { item: i.clone() }
        
                        },
                    }
                },
            }
        }
    }
}
