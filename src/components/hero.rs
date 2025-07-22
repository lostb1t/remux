use crate::components;
use crate::hooks;
use crate::media;
use crate::server;
use crate::utils;
use crate::views;
use crate::Route;
use chrono::Datelike;
use dioxus::prelude::*;
use dioxus_free_icons::icons::io_icons::{
    IoAirplane, IoEye, IoEyeOutline, IoHeart, IoHeartOutline,
};
use dioxus_logger::tracing::{debug, info, trace, Level};
use rand::Rng;
use tracing_subscriber::field::debug;

use dioxus_free_icons::Icon;
use dioxus_time::use_debounce;
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone, PartialEq, Props)]
pub struct HeroListProps {
    pub title: Option<String>,
    pub query: server::MediaQuery,
    //#[props(optional)]
    //pub items: Option<Vec<media::Media>>,
}

#[component]
pub fn HeroList(props: HeroListProps) -> Element {
    let server = hooks::consume_server().expect("missing server");
    let query = props.query.clone();
    let mut index = use_signal(|| 0_usize);

    let media_items = {
        let server = server.clone();
        let query = query.clone();

        utils::use_paginated_resource(10, move |limit, offset| {
            let server = server.clone();
            let mut paged_query = query.clone();
            paged_query.offset = offset as u32;
            // paged_query.limit = 1;
            debug!("Hero: Fetching items with offset: {}", paged_query.offset);
            async move { Ok(crate::server::get_media_cached(server, &paged_query).await?) }
        })
        .suspend()?
    };

    let list = media_items().items.read().clone();

    if list.is_empty() {
        return rsx! {};
    }

    let scroll_to_index = {
        let mut index = index.clone();
        move |i: usize| {
            index.set(i);
        }
    };

    rsx! {
        div { class: "relative mb-6",
            components::CarouselList {
                items: list.clone(),
                index: index.clone(),
                on_load_more: Some(
                    EventHandler::new(move |_| {
                        if !*media_items().is_loading.read() {
                            media_items().load_next();
                        }
                    }),
                ),
                render_item: move |item: &media::Media| rsx! {
                    div { class: "flex-shrink-0 w-full snap-start",
                        HeroItem { item: item.clone() }
                    }
                },
                        // render_item: move |item: &media::Media, idx: String| rsx! {
            //     div {
            //         id: idx,
            //         class: "flex-shrink-0 w-full snap-start",
            //         HeroItem { item: item.clone() }
            //     }
            // },
            }
            PaginationDots {
                list_len: list.len(),
                index: index.clone(),
                max_dots: 10,
                scroll_to_index: Callback::new(scroll_to_index.clone()),
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct HeroItemProps {
    pub item: media::Media,

    #[props(default = false)]
    pub detail: bool,
    // pub id: String,
    //   pub disable_links: bool,
}

use super::FadeInImage;

#[component]
pub fn HeroItem(props: HeroItemProps) -> Element {
    // info!("HeroItem: {:?}", &item);
    let mut player = components::video::use_video_player();
    let server = hooks::consume_server().unwrap();
    let item = props.item.clone();
    let mut is_favorite = use_signal(|| {
        item.user_data
            .clone()
            .map(|x| x.is_favorite)
            .unwrap_or(false)
    });
    let mut is_watched = use_signal(|| {
        item.user_data
            .clone()
            .map(|x| x.is_watched)
            .unwrap_or(false)
    });
    //let binding = server.read();
    //let server = binding.as_ref().unwrap().clone();
    let mut load_logo = use_signal(|| false);
    // debug!("HeroItem: item: {:?}", &item.backdrop);
    let backdrop_url = match &item.backdrop {
        Some(backdrop) => server.image_url(&item, media::ImageType::Backdrop),
        None => server.image_url(&item, media::ImageType::Poster),
    };

    let logo_url = server.image_url(&item, media::ImageType::Logo);

    let logo_src_resource = {
        let item = item.clone();
        let server = server.clone();
        use_resource(move || {
            let server = server.clone();
            let item = item.clone();
            //to_owned![item, server];
            async move {
                if !load_logo() || item.logo.is_none() {
                    return None;
                };
                let logo_url = server.image_url(&item, media::ImageType::Logo);
                utils::fetch_and_trim_base64(&logo_url.unwrap()).await
            }
        })
    };

    let mut subtitle_vec = Vec::new();

    subtitle_vec.push(item.media_type.to_string());

    if let Some(date) = item.release_date {
        subtitle_vec.push(date.year().to_string());
    }

    subtitle_vec.extend(item.genres.clone());
    //let logo_src = logo_src_resource().unwrap_or_default();

    rsx! {
        div {
            class: "relative min-h-[80vh] max-h-[80vh] lg:min-h-140 lg:max-h-140 w-full text-white overflow-hidden",
            onvisible: move |evt| {
                let data = evt.data();
                if let Ok(is_intersecting) = data.is_intersecting() {
                    load_logo.set(true);
                }
            },
            Link {
                to: Route::MediaDetailView {
                    media_type: item.media_type.clone(),
                    id: item.id.clone(),
                },
                class: "absolute inset-0 w-full h-full block",

                FadeInImage {
                    src: backdrop_url.unwrap(),
                    alt: item.title.clone(),
                    class: "absolute inset-0 w-full object-cover h-full",
                    attr: vec![],
                }
                div { class: "absolute bottom-0 left-0 right-0 h-1/2 bg-gradient-to-t from-neutral-900 via-neutral-900/100 to-transparent pointer-events-none" }
            }

            // Overlay gradient


            // Foreground content (text + play)
            div {
                class: "sidebar-offset absolute w-full bottom-0 lg:min-w-md lg:max-w-md flex flex-col justify-center p-6 space-y-4",

                Link {
                    to: Route::MediaDetailView {
                        media_type: item.media_type.clone(),
                        id: item.id.clone(),
                    },
                    class: "space-y-4 block",



                    match logo_src_resource() {
                        Some(Some(logo)) => rsx! {
                            FadeInImage {
                                src: logo,
                                //src:  logo_url,
                                class: "w-full max-h-24 lg:max-h-42 object-contain",
                                //class: "invert brightness-0",
                                //attr: vec![],
                            }
                        },
                        Some(None) => rsx! {
                            h1 { class: "text-4xl font-bold", "{item.title}" }
                        },
                        None => rsx! {},
                    }

                    //if !item.genres.is_empty() {
                    p { class: "text-sm ml-6 mr-6 text-center truncate font-medium",
                        "{subtitle_vec.join(\" · \")}"
                    }
                                // }
                }


                //   div {
                //     class: "w-full",
                //if props.detail {

                div { class: "flex w-full gap-2 items-center",

                    components::PlayButton { class: "flex-1 h-10 p-0", media_item: item.clone() }

                    components::Button {
                        variant: components::ButtonVariant::Secondary,
                        onclick: {
                            to_owned![item, server];
                            move |_| {
                                to_owned![item, server];
                                let fav = is_favorite();
                                is_favorite.set(!fav);
                                spawn(async move {
                                    server.is_favorite(!fav, &item).await;
                                });
                            }
                        },
                        //   class: "flex-none",
                        class: "flex-none flex items-center justify-center w-10 h-10",
                        //if let Some(data) = item.user_data {
                        super::ToggleIcon {
                            width: 18,
                            height: 18,
                            fill: "white",
                            icon: IoHeartOutline,
                            icon_active: IoHeart,
                            active: *is_favorite.read(),
                        }
                    }

                    components::Button {
                        variant: components::ButtonVariant::Secondary,
                        onclick: {
                            to_owned![item, server];
                            move |_| {
                                to_owned![item, server];
                                spawn(async move {
                                    let watched = is_watched();
                                    is_watched.set(!watched);
                                    server.is_watched(!watched, &item).await;
                                });
                            }
                        },
                        //   class: "flex-none",
                        class: "flex-none w-10 h-10 flex items-center justify-center",
                        //if let Some(data) = item.user_data {
                        super::ToggleIcon {
                            width: 18,
                            height: 18,
                            fill: "white",
                            icon: IoEye,
                            icon_active: IoEyeOutline,
                            active: *is_watched.read(),
                        }
                    }
                }
                        //  } else {
            //     components::PlayButton { class: "w-full", media_item: item.clone() }
            //}

            }
        



        }



        // }

        // Description
        if props.detail {
            div { class: "sidebar-offset px-6 space-y-4 flex flex-col",

                if item.description.is_some() {
                    Link {
                        to: Route::MediaDetailView {
                            media_type: item.media_type.clone(),
                            id: item.id.clone(),
                        },
                        // class: "pointer-events-auto",
                        div { class: "pt-2 line-clamp-4 text-sm font-medium",
                            "{item.description.as_deref().unwrap()}"
                        }
                    }
                }

                components::TagsDisplay { media_item: item }
            }
        }
    }
}

#[derive(Props, PartialEq, Clone)]
pub struct PaginationDotsProps {
    pub list_len: usize,
    pub index: Signal<usize>,
    pub max_dots: usize,
    pub scroll_to_index: Callback<usize>,
}

#[component]
pub fn PaginationDots(props: PaginationDotsProps) -> Element {
    let current_index = *props.index.read();
    let total_items = props.list_len;
    let max_dots = props.max_dots;

    let half = max_dots / 2;
    let mut start = current_index.saturating_sub(half);
    let mut end = (start + max_dots).min(total_items);

    if end - start < max_dots && total_items >= max_dots {
        start = total_items - max_dots;
        end = total_items;
    }

    let pagination_dots = (start..end).map(|i| {
        let active = current_index == i;
        let scroll_to_index = props.scroll_to_index.clone();
        let mut index = props.index.clone();

        rsx! {
            div {
                class: "p-2 pl-2 pr-1",
                onclick: move |_| {
                    index.set(i);
                    scroll_to_index.call(i);
                },
                div {
                    class: "w-2 h-2 rounded-full transition-all cursor-pointer",
                    class: if active { "bg-white w-5" } else { "bg-white/50" },
                }
            }
        }
    });

    rsx! {
        div { class: "sidebar-offset flex justify-center items-center",
            {pagination_dots.collect::<Vec<_>>().into_iter()}
        }
    }
}

#[derive(Props, PartialEq, Clone)]
pub struct TagsDisplayProps {
    // pub ratings: Vec<media::Rating>,
    pub media_item: media::Media,
}

#[component]
pub fn TagsDisplay(props: TagsDisplayProps) -> Element {
    let ratings = props.media_item.ratings.clone();

    // if ratings.is_empty() {
    //     return rsx! {};
    // }

    rsx! {
        div { class: "flex flex-row items-center gap-2",

            if let Some(official_rating) = &props.media_item.official_rating {
                // rsx! {
                div { class: "text-sm font-medium flex items-center gap-1", "{official_rating}" }
            }

            if let Some(runtime) = &props.media_item.runtime_seconds {
                // rsx! {
                div { class: "text-sm font-medium flex items-center gap-1",
                    "{&props.media_item.formatted_runtime()}"
                }
            }

            // }
            {ratings.iter().filter_map(|rating| { Some(rsx! {
                div { class: "text-sm font-medium flex items-center gap-1",
                    {
                        rsx! {
                            //div {
                            // class: "bg-green-500 inline-block",
                            img {
                                class: "h-3 img-gray",
                                //  style: "filter: invert(1); filter: invert(0.5) sepia(1) saturate(5) hue-rotate(175deg)",
                                src: "{rating.icon_path()}",
                            }
                            "{rating.format_score()}"
                        }
                    }

    
                }
            }) })}
        }
    }
}
