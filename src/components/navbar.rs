use crate::components::video::VideoPlayerCallback;
use crate::Route;
use dioxus::prelude::*;
use dioxus_elements::div;
use dioxus_motion::prelude::*;

//const NAVBAR_CSS: Asset = asset!("/assets/styling/navbar.css");

#[component]
pub fn Navbar() -> Element {
    rsx! {
        //document::Link { rel: "stylesheet", href: NAVBAR_CSS }

      // div {
     //       id: "navbar",
     //       class: "btm-nav",
     //       Link {
     //          to: Route::Home {},
     //           "Home"
     //       }
     //       Link {
     //           to: Route::Settings { },
      //           "Settings"
      //      }
      //  }
        div {
            class: "flex justify-center bg-base-300",
            div {
                class: "w-full",
                AnimatedOutlet::<Route> {}
            }
        }
        VideoPlayerCallback{}
    }
}
