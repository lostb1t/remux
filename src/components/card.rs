use crate::Route;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::Display as EnumDisplay;
use strum_macros::{EnumString,EnumIter};

#[derive(Clone, Hash, EnumDisplay, Serialize, Deserialize,EnumIter, EnumString, Debug, Eq, PartialEq)]
pub enum CardVariant {
    Poster,
    Square,
    Landscape,
    Hero
}

impl Default for CardVariant {
    fn default() -> Self {
        Self::Poster
    }
}

fn image_class(variant: &CardVariant) -> &'static str {
    match variant {
        CardVariant::Poster => "w-26 aspect-[2/3]",
        CardVariant::Square => "w-45 h-45 aspect-square",
        CardVariant::Landscape => "w-60 aspect-video",
        CardVariant::Hero => "w-[calc(100vw-2.5rem)] max-h-110 max-w-100 aspect-[5/6]",
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
        div { 
            class: "flex-none relative shrink-0 {image_class(&props.variant)} {props.class}",
           super::FadeInImage {
                src: "{props.image}",

                class: "rounded-lg w-full h-auto object-cover {image_class(&props.variant)}",
            }
            {props.children}
        }
    };

    match &props.to {
        Some(route) => rsx! {
            Link { to: route.clone(), class: "", {content} }
        },
        None => content,
    }
}
