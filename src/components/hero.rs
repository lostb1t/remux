use dioxus::prelude::*;
use crate::components;
use crate::Route;


#[component]
pub fn Hero() -> Element {
    let mut player = components::video::use_video_player();
    

    rsx! {
        div {
            class: "relative min-h-[80vh] max-h-[80vh] h-full w-full bg-cover bg-center text-white",
            style: "background-image: url('https://image.tmdb.org/t/p/original/6WqqEjiycNvDLjbEClM1zCwIbDD.jpg');",

            // Top gradient overlay
            div {
                class: "absolute inset-0 bg-gradient-to-t from-black via-black/50 to-transparent z-0"
            }

            // Back button + category
            div {
                class: "absolute top-6 left-4 z-10 flex items-center gap-2",
                button {
                    class: "px-4 py-1.5 rounded-full border border-white/50 text-white/80 bg-black/40 text-sm font-semibold backdrop-blur-sm hover:bg-white/10",
                    "Films"
                }
                button {
                    class: "px-4 py-1.5 rounded-full border border-white/50 text-white/80 bg-black/40 text-sm font-semibold backdrop-blur-sm hover:bg-white/10",
                    "Shows"
                }
                button {
                    class: "px-4 py-1.5 rounded-full border border-white/50 text-white/80 bg-black/40 text-sm font-semibold backdrop-blur-sm hover:bg-white/10",
                    "Genre"
                }
                Link {
                    to: Route::Settings { },
                    class: "px-4 py-1.5 rounded-full border border-white/50 text-white/80 bg-black/40 text-sm font-bold backdrop-blur-sm hover:bg-white/10",
                    "⚙" // or use an icon component
                }

            }

            div {
                class: "relative z-10 flex flex-col justify-end grow h-full min-h-[inherit] p-6 sm:p-12",
                h1 {
                    class: "text-4xl font-bold mb-4",
                    "28 years later"
                }

                p {
                    class: "text-lg font-medium mb-6",
                    "Movie · Sci-Fi · Adventure"
                }

                components::Button {
                    //variant: "primary",
                    onclick: move |_| async move {
                       player.with_mut(|s| s.src = Some("yogo".to_string()))
                    },
                    "play"
                }

                // Pagination dots
                //div {
                //   class: "flex justify-center items-center mt-6 gap-2",
                  // (0..6).map(|i| rsx! {
                  //      div {
                  //         class: "w-2 h-2 rounded-full bg-white",
                  //         "yo"
                  //      }
                  // })
              //  }
            }
        }
        
        
    }
}