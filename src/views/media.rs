use crate::components;
use crate::Route;
use dioxus::prelude::*;
use dioxus_router::prelude::*;



#[derive(PartialEq, Props, Clone)]
pub struct MediaDetailViewProps {
    pub id: u32,
    pub media_type: crate::media::MediaType,
}

#[component]
pub fn MediaDetailView(props: MediaDetailViewProps) -> Element {
    let mut player = components::video::use_video_player();

    rsx! {
      div {
          class: "absolute top-6 left-4 flex items-center gap-2 z-50 pt-[env(safe-area-inset-top)] px-[env(safe-area-inset-left)]",
                Link {
                    to: Route::Home {},
                    class: "px-4 py-1.5 rounded-full border border-white/50 text-white/80 bg-black/40 text-sm font-bold backdrop-blur-sm hover:bg-white/10",
                    "Back"
                }
                button {
                    class: "px-4 py-1.5 rounded-full border border-white/50 text-white/80 bg-black/40 text-sm font-semibold backdrop-blur-sm hover:bg-white/10",
                    "Favorite"
                }
            }
        div {
            class: "relative min-h-[80vh] max-h-[80vh] h-full w-full bg-cover bg-center text-white",
            style: "background-image: url('https://image.tmdb.org/t/p/original/ormMH10vPB9gxAvYwhP93uqfeuX.jpg');",

            // Top gradient overlay
            div {
                class: "absolute inset-0 bg-gradient-to-t from-black via-black/50 to-transparent"
            }

            div {
                class: "relative z-10 flex flex-col justify-end grow h-full min-h-[inherit] p-6 sm:p-12",
                h1 {
                    class: "text-4xl font-bold mb-4",
                    "29 years later"
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

            }
        }


    }
}
