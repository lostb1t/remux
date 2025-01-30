use crate::components;
use dioxus::prelude::*;
use crate::clients;
// use jellyfin_api;
use jellyfin_api;
use remux_web::hooks::*;
use daisy_rsx;
use dioxus_logger::tracing::{info};

#[component]
pub fn Home() -> Element {
    rsx! {
        // components::Hero {}
        components::Button { "Click me" }
        Media {}
    }
}

#[component]
fn Media() -> Element {
    let mut app = use_app();

    info!("{:?}", &app.user);
    // Fetch the top 10 stories on Hackernews
    let media = use_resource(move || clients::remux::get_media());
    // let media = use_resource(move || jellyfin_api::Client);

    // check if the future is resolved
    match &*media.read_unchecked() {
        Some(Ok(list)) => {
            // if it is, render the stories
            rsx! {
                div {
                    class: "carousel w-full",
                    // iterate over the stories with a for loop
                    for media in list {
                        div {
                            class: "carousel-item",
                            daisy_rsx::Card {
                                class: "w-28",
                                daisy_rsx::CardHeader {
                                    title: "{media.name}"
                                }
                            }
                        }
                    }
                }
            }
        }
        Some(Err(err)) => {
            // if there was an error, render the error
            rsx! {"An error occurred while fetching media {err}"}
        }
        None => {
            // if the future is not resolved yet, render a loading message
            rsx! {"Loading items"}
        }
    }
}