use crate::components;
use dioxus::prelude::*;
use crate::clients;

use daisy_rsx;

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
    // Fetch the top 10 stories on Hackernews
    let media = use_resource(move || clients::remux::get_media());

    // check if the future is resolved
    match &*media.read_unchecked() {
        Some(Ok(list)) => {
            // if it is, render the stories
            rsx! {
                div {
                    // iterate over the stories with a for loop
                    for media in list {
                        // render every story with the StoryListing component
                        // StoryListing { story: story.clone() }
                        daisy_rsx::Card {
                            daisy_rsx::CardHeader {
                                title: "{media.name}"
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
            rsx! {"Loading itmediaems"}
        }
    }
}