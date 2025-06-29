use crate::media;
use dioxus::prelude::*;
//use dioxus_lazy::{lazy, List};
use dioxus_router::prelude::*;
use crate::Route;

#[derive(PartialEq, Clone)]
pub enum Style {
    Poster,
    Landscape,
}

impl Default for Style {
    fn default() -> Self {
        Self::Poster
    }
}

#[derive(Clone, Props, PartialEq)]
pub struct MediaListProps {
    #[props(default)]
    pub style: Style,
    pub title: Option<String>,
    pub items: Vec<media::Media>,
}

#[component]
pub fn MediaList(props: MediaListProps) -> Element {
    rsx! {
        div {
            class: "px-4",
         if let Some(title) = props.title {
            h2 {
                class: "text-xl font-bold text-white mb-4",
                "{title}"
            }
          }
          
          

            // Scrollable row
            div {
                class: "flex overflow-x-auto gap-3 pb-3 scroll-smooth",
                style: "scrollbar-width: none; -ms-overflow-style: none;",
                // Hide scrollbar on WebKit
                div { class: "hidden", style: "-webkit-overflow-scrolling: touch;" }

                // Poster cards
                PosterCard {
                    title: "SEE",
                    image: "https://image.tmdb.org/t/p/original/361hRZoG91Nw6qXaIKuGoogQjix.jpg",
                    to: Route::MediaDetailView {
                        media_type: media::MediaType::Movie,
                        id: 50,
                    }
                }
                PosterCard {
                    title: "FOR ALL MANKIND",
                    image: "https://image.tmdb.org/t/p/original/361hRZoG91Nw6qXaIKuGoogQjix.jpg"
                }
                PosterCard {
                    title: "MASTERS OF THE AIR",
                    image: "https://image.tmdb.org/t/p/original/361hRZoG91Nw6qXaIKuGoogQjix.jpg"
                }
                PosterCard {
                    title: "BUCCANEERS",
                    image: "https://image.tmdb.org/t/p/original/361hRZoG91Nw6qXaIKuGoogQjix.jpg"
                }
            }
        }
    }
}

#[component]
fn MediaPosterCard(item: media::Media) -> Element {
    rsx! {
      
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct PosterCardProps {
    pub title: &'static str,
    pub image: &'static str,
    #[props(optional)]
    pub to: Option<Route>,
}

#[component]
pub fn PosterCard(props: PosterCardProps) -> Element {
    let content = rsx! {
        div {
            class: "flex-none w-25 shrink-0",
            img {
                src: "{props.image}",
                alt: "{props.title}",
                class: "rounded-xl w-full h-auto object-cover"
            }
        }
    };

    match &props.to {
        Some(route) => rsx! {
            Link {
                to: route.clone(),
                class: "block",
                {content}
            }
        },
        None => content,
    }
}