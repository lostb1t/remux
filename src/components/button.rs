use crate::components;
use crate::hooks;
use crate::media;
use dioxus::prelude::*;
use dioxus_free_icons::icons::fa_solid_icons::FaPlay;
use dioxus_free_icons::Icon;
use dioxus_logger::tracing::{debug, error, info};
//use gloo_timers::future::sleep;

use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Outline,
    Ghost,
    Link,
    Destructive,
}

impl Default for ButtonVariant {
    fn default() -> Self {
        Self::Primary
    }
}

/// Button size options
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonSize {
    Small,
    Medium,
    Large,
}

impl Default for ButtonSize {
    fn default() -> Self {
        Self::Medium
    }
}

#[derive(Clone, Props, PartialEq)]
pub struct ButtonProps {
    children: Element,

    #[props(default)]
    variant: ButtonVariant,

    #[props(default)]
    size: ButtonSize,

    #[props(default)]
    class: String,

    onclick: Option<EventHandler<MouseEvent>>,

    //#[props(extends = GlobalAttributes)]
    #[props(extends = button, extends = GlobalAttributes)]
    attr: Vec<Attribute>,
}

#[component]
pub fn Button(props: ButtonProps) -> Element {
    let class = match props.variant {
        ButtonVariant::Primary => "p-3 rounded-lg bg-white text-black text-xs font-semibold border border-gray-300 ",
        ButtonVariant::Secondary => "p-3 rounded-lg bg-neutral-800 text-white text-sm font-bold",
        ButtonVariant::Outline => "p-3 rounded-lg border border-gray-300 hover:bg-gray-100 text-sm font-semibold",
        _ => "p-3 rounded-lg bg-white text-black border border-gray-300 hover:bg-gray-100 text-sm font-semibold",
    };

    rsx!(
        button {
            //class,

            class: "flex items-center justify-center gap-2 leading-none cursor-pointer {props.class} {class}",
            onclick: move |event| {
                if let Some(f) = props.onclick.as_ref() {
                    f(event)
                }
            },
            //..attr,
            {props.children}
        }
    )
}

#[derive(Props, PartialEq, Clone)]
pub struct PlayButtonProps {
    media_item: media::Media,
    #[props(default)]
    class: String,
    #[props(extends = button, extends = GlobalAttributes)]
    attr: Vec<Attribute>,
}

#[component]
pub fn PlayButton(props: PlayButtonProps) -> Element {
    let server = hooks::consume_server().expect("uhu");
    let mut sheet_open = use_signal(|| false);
    let mut media_item = props.media_item.clone();
    let mut player = super::use_video_player();
    let is_movie_or_episode = matches!(
        media_item.media_type,
        media::MediaType::Movie | media::MediaType::Episode
    );
    let has_multiple_sources = media_item.media_sources.len() > 1;
    let should_show_sheet = is_movie_or_episode && has_multiple_sources;
    //let should_show_sheet = is_movie_or_episode && has_multiple_sources;

    let nextup_items = {
        to_owned![server, media_item];
        use_resource(move || {
            to_owned![server, media_item];

            async move { server.nextup(&media_item).await }
        })
    };

    rsx! {

        Button {
            class: props.class,
            onclick: {
                to_owned![player, media_item];
                move |_| {
                  if media_item.is_series() {
                        let i = nextup_items.read();
                        if let Some(Ok(items)) = i.as_ref() {
                            if let Some(first) = items.first() {
                                media_item = first.clone();
                            }
                        }
                    }
                    if should_show_sheet {
                        sheet_open.set(true);
                    } else {
                        player.set_media(media_item.clone(), None);
                        player.play();
                    }
                }
            },
            attr: props.attr.clone(),
            Icon {
                width: 16,
                height: 16,
                fill: "black",
                icon: FaPlay,
            }
            "Play"
        }

        if *sheet_open.read() {
            super::Sheet {
                open: sheet_open,
                children: rsx! {
                    components::List {
                        for source in media_item.media_sources.clone() {
                            components::ListItem {
                                a {
                                    class: "cursor-pointer",
                                    onclick: {
                                        to_owned![player, media_item];
                                        move |_| {
                                            sheet_open.set(false);
                                            player.set_media(media_item.clone(), Some(source.clone()));
                                            player.play();
                                        }
                                    },
                                    "{source.name}"
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}
