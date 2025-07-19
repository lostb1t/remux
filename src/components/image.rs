use crate::media;
use dioxus::prelude::*;
//use dioxus_lazy::{lazy, List};
use crate::hooks;
use crate::sdks;
use crate::server;
use crate::Route;
use dioxus_logger::tracing::{debug, info};
//use dioxus_router::prelude::*;
use std::sync::Arc;
//use dioxus_lazy::

#[derive(Props, Clone, PartialEq)]
pub struct FadeInImageProps {
    src: String,
    #[props(optional)]
    class: Option<String>,
    #[props(extends = GlobalAttributes)]
    attr: Vec<Attribute>,
}

#[component]
pub fn FadeInImage(props: FadeInImageProps) -> Element {
    let mut loaded: Signal<bool> = use_signal(|| false);

    let class = format!(
        "transition-opacity duration-700 ease-in {} {}",
        if *loaded.read() {
            "opacity-100"
        } else {
            "opacity-0"
        },
        props.class.as_deref().unwrap_or("")
    );

    rsx! {

        img {
            loading: "lazy",
            src: "{props.src}",
            // width: "1920",
            //height: "1080",
            class,
            onload: move |_| loaded.set(true),
            ..props.attr,
        }
    }
}
