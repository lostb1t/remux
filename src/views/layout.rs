use crate::components;
use crate::hooks;
use crate::media;
use crate::Route;
use crate::ServerProvider;
use dioxus::prelude::*;
use dioxus_elements::div;
use dioxus_logger::tracing::{info, Level};
//use dioxus_motion::prelude::*;

#[component]
fn LoadingProvider(children: Element) -> Element {
    rsx! {
        SuspenseBoundary {
            fallback: |context: SuspenseContext| rsx! {
                div {
                    crate::Loading { class: "sidebar-offset bg-base-100/100" }
                }
            },
            {children}
        }
    }
}

#[component]
pub fn AuthenticatedLayout() -> Element {
    rsx! {
        ServerProvider { Outlet::<Route> {} }
    }
}

#[component]
pub fn MainLayout() -> Element {
    let mut player = components::use_video_player();
    rsx! {
        div { class: "pb-[calc(2rem+env(safe-area-inset-bottom))] md:pb-[env(safe-area-inset-bottom)] min-h-screen flex w-full",

            // Sidebar with fixed width on lg+
            div { class: "hidden fixed md:block w-50 z-30 flex-none min-h-screen", components::Sidebar {} }

            // Main content fills the rest
            div { 
                // class: "flex-auto min-w-0 pb-10 lg:pb-6 md:ml-50 md:w-[calc(100%-13rem)] ",
                class: "flex-auto min-h-screen min-w-0 pb-10 lg:pb-6",

                LoadingProvider { Outlet::<Route> {} }
            }
        }

        div { class: "md:hidden", components::BottomNavbar {} }
        if *player.visible.read() {
            components::VideoPlayer {}
        }
    }
}

/// Offset for sidebar and add safe area
#[component]
pub fn SafeSpaceLayout() -> Element {
    rsx! {
        div { class: "sidebar-offset pt-[env(safe-area-inset-top)] p-6", Outlet::<Route> {} }
    }
}

#[component]
pub fn HomeMenu() -> Element {
    let mut genres_open = use_signal(|| false);
    let mut home_filter = hooks::use_home_filter();
    let mut genre = home_filter.genre;
    let media_type = home_filter.media_type.read();
    let is_movie = media_type.as_ref() == Some(&media::MediaType::Movie);
    let is_series = media_type.as_ref() == Some(&media::MediaType::Series);
    let has_genre = genre.read().is_some();

    rsx! {
        div { class: "sidebar-offset absolute left-6 top-6 flex items-center gap-2 z-10 pt-[env(safe-area-inset-top)] px-[env(safe-area-inset-left)]",

            button {
                class: "px-4 py-1.5 rounded border-white/50 text-sm font-semibold backdrop-blur-sm hover:bg-white/10",
                class: if is_movie { "bg-white/40 text-black/90" } else { "bg-black/40  text-white/80" },
                onclick: move |_| {
                    home_filter
                        .media_type
                        .set(if is_movie { None } else { Some(media::MediaType::Movie) });
                },
                "Films"
            }

            button {
                class: "px-4 py-1.5 rounded border-white/50 text-white/80 bg-black/40 text-sm font-semibold backdrop-blur-sm hover:bg-white/10",
                class: if is_series { "bg-white/40 text-black/90" } else { "text-white/80" },
                onclick: move |_| {
                    home_filter
                        .media_type
                        .set(if is_series { None } else { Some(media::MediaType::Series) });
                },
                "Shows"
            }


            if has_genre {
                div { class: "inline-flex rounded overflow-hidden backdrop-blur-sm",

                    button {
                        class: "px-4 py-1.5 bg-white/40 text-black/90 text-sm font-semibold",
                        onclick: move |_| genres_open.set(true),
                        match genre.read().clone() {
                            Some(g) => g.name,
                            None => "Genre".to_string(),
                        }
                    }

                    button {
                        class: "px-3 py-1.5 bg-black/40 text-white/70 text-sm font-semibold",
                        onclick: move |_| genre.set(None),
                        "X"
                    }
                }
            } else {
                button {
                    class: "px-4 py-1.5 rounded border-white/50 text-white/80 bg-black/40 text-sm font-semibold backdrop-blur-sm hover:bg-white/10",
                    onclick: move |_| genres_open.set(true),
                    "Genre"
                }
            }



            components::Sheet { title: "Genres", open: genres_open,
                GenreList { open: genres_open }
            }
        }
    }
}

#[derive(Props, PartialEq, Clone)]
pub struct GenreListProps {
    pub open: Signal<bool>,
}

#[component]
pub fn GenreList(props: GenreListProps) -> Element {
    let mut open = props.open;
    let server = hooks::consume_server().unwrap();
    //let mut genres = hooks::use_genres(server);
    let mut home_filter = hooks::use_home_filter();
    //let mut genres_open = use_signal(|| false);
    //let genres_value = genres.read();

    let genres = use_resource(move || {
        let server = server.clone();
        async move { server.get_genres().await }
    });

    let genres = genres.read();
    match &*genres {
        None => rsx! {
            div { class: "p-4 w-full h-full min-h-10",
                crate::Loading { transparant: true }
            }
        },
        Some(Err(e)) => rsx! {
            div { class: "p-4 w-full min-h-10v",

                p { class: "text-red-500", "Error: {e}" }
            }
        },

        Some(Ok(data)) => rsx! {
            div { class: "p-4 w-full h-full",
                components::List {
                    {
                        data.iter()
                            .map(|g| {
                                let genre = g.clone();
                                rsx! {
                                    components::ListItem {
                                        button {
                                            onclick: move |_| {
                                                open.set(false);
                                                home_filter.genre.set(Some(genre.clone()));
                                            },
                                            "{genre.name}"
                                        }
                                    }
                                }
                            })
                    }
                }
            }
        },
    }
}

// #[component]
// pub fn HomeLayout() -> Element {
//     rsx! {
//         div { class: "bg-neutral-900 min-h-screen flex flex-col relative",
//             div { class: "flex-1 flex justify-center",
//                 div { class: "w-full",
//                 TopNavbar{}
//                     ServerProvider {
//                         //HomeMenu {}
//                        // AnimatedOutlet::<Route> {}
//                         Outlet::<Route> {}
//                     }
//                 }
//             }

//             components::BottomNavbar {}
//             VideoPlayerCallback {}
//         }
//     }
// }

#[component]
pub fn UnauthenticatedLayout() -> Element {
    rsx! {
        div { class: "flex justify-center",
            div { class: "w-full h-full", Outlet::<Route> {} }
        }
    }
}
