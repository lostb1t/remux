use crate::Route;
use dioxus::prelude::*;
use dioxus_elements::div;

const NAVBAR_CSS: Asset = asset!("/assets/styling/navbar.css");

#[component]
pub fn Navbar() -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: NAVBAR_CSS }

        div {
            id: "navbar",
            class: "navbar",
            Link {
                to: Route::Home {},
                "Home"
            }
            Link {
                to: Route::Settings { },
                 "Settings"
            }
        }
        div {
            class: "flex justify-center bg-base-300",
            div {
                class: "w-full m-4",
                Outlet::<Route> {}
            }
        }
    }
}
