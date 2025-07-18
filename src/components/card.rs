use crate::Route;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::Display as EnumDisplay;
use strum_macros::EnumString;

#[derive(Clone, Hash, EnumDisplay, Serialize, Deserialize, EnumString, Debug, PartialEq)]
pub enum CardVariant {
    Poster,
    Square,
    Landscape,
}

impl Default for CardVariant {
    fn default() -> Self {
        Self::Poster
    }
}

fn image_class(variant: &CardVariant) -> &'static str {
    match variant {
        CardVariant::Poster => "w-25 aspect-[2/3]",
        CardVariant::Square => "aspect-square",
        CardVariant::Landscape => "w-35 aspect-video",
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct CardProps {
    pub title: Option<String>,
    pub image: String,
    #[props(optional)]
    pub to: Option<Route>,
    #[props(default = CardVariant::Poster)]
    pub variant: CardVariant,
    #[props(optional, default = "".to_string())]
    pub class: String,
    //   #[props(extends = div)]
    //   pub extra: Option<()>,
    pub children: Element,
}

#[component]
pub fn Card(props: CardProps) -> Element {
    let content = rsx! {
        div { class: "flex-none relative shrink-0 {image_class(&props.variant)} {props.class}",
            // ..props.extra,
            super::FadeInImage {
                src: "{props.image}",
                //alt: "{props.title}",
                class: "rounded w-full h-auto object-cover",
            }
            {props.children}
                //div { class: "absolute inset-0 h-full w-full pointer-events-none", {props.children} }
        }
    };

    match &props.to {
        Some(route) => rsx! {
            Link { to: route.clone(), class: "w-full h-full", {content} }
        },
        None => content,
    }
}
