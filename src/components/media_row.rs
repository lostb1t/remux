use dioxus::prelude::*;

#[component]
pub fn MediaRow() -> Element {
    rsx! {
        div {
            class: "px-4",
            h2 {
                class: "text-xl font-bold text-white mb-4",
                "Recently Added"
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
                    image: "https://image.tmdb.org/t/p/original/361hRZoG91Nw6qXaIKuGoogQjix.jpg"
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
fn PosterCard(title: &'static str, image: &'static str) -> Element {
    rsx! {
        div {
            class: "flex-none w-25 shrink-0",
            img {
                src: "{image}",
                alt: "{title}",
                class: "rounded-xl w-full h-auto object-cover"
            }
        }
    }
}