use crate::Route;
use dioxus::prelude::*;

use dioxus_free_icons::icons::hi_solid_icons::{HiCog, HiHome, HiSearch};
use dioxus_free_icons::Icon;

#[component]
pub fn BottomNavbar() -> Element {
    let mut show_search = use_signal(|| false);
    let mut search_query = use_signal(|| String::new());
    let navigator = use_navigator();

    rsx! {
        div { class: "fixed bottom-3 left-0 w-full z-100",
            div { class: "flex justify-center",
                ul { class: "flex px-4 py-2 gap-3 rounded-lg bg-neutral-700 shadow-md items-center",

                    li {
                        Link { to: Route::Home {},
                            Icon {
                                width: 28,
                                height: 28,
                                fill: "white",
                                icon: HiHome,
                            }
                        }
                    }

                    li {
                        Link { to: Route::Settings {},
                            Icon {
                                width: 28,
                                height: 28,
                                fill: "white",
                                icon: HiCog,
                            }
                        }
                    }

                    li {
                        button {
                            r#type: "button",
                            onclick: move |_| show_search.set(!show_search()),
                            Icon {
                                class: "mt-2",
                                width: 28,
                                height: 28,
                                fill: "white",
                                icon: HiSearch,
                            }
                        }
                    }

                    // Animated search input
                    li {
                        div {
                            class: format_args!(
                                "transition-all duration-300 overflow-hidden {}",
                                if show_search() { "w-40 opacity-100 ml-2" } else { "w-0 opacity-0 ml-0" },
                            ),
                            form {
                                onsubmit: move |evt| {
                                    if !search_query().is_empty() {
                                        navigator
                                            .push(Route::SearchView {
                                                query: search_query().clone(),
                                            });
                                    }
                                },
                                input {
                                    class: "bg-neutral-600 text-white px-2 py-1 rounded w-full",
                                    r#type: "text",
                                    placeholder: "Search...",
                                    value: "{search_query}",
                                    oninput: move |e| search_query.set(e.value().clone()),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn BottomNavbarOld() -> Element {
    rsx! {
        div { class: "fixed bottom-2 left-0 w-full z-50",
            div { class: "flex justify-center",
                ul { class: "flex px-4 py-2 gap-4 rounded-lg bg-neutral-700 shadow-md",

                    li {
                        Link {
                            // class: "p-4",
                            to: Route::Home {},
                            svg {
                                xmlns: "http://www.w3.org/2000/svg",
                                class: "h-6 w-6",
                                fill: "none",
                                view_box: "0 0 24 24",
                                stroke: "currentColor",
                                path {
                                    stroke_linecap: "round",
                                    stroke_linejoin: "round",
                                    stroke_width: "2",
                                    d: "M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6",
                                }
                            }
                        }
                    }

                    li {
                        Link {
                            //  class: "p-4",
                            to: Route::Settings {},
                            svg {
                                xmlns: "http://www.w3.org/2000/svg",
                                class: "h-6 w-6",
                                fill: "none",
                                view_box: "0 0 24 24",
                                stroke: "currentColor",
                                path {
                                    stroke_linecap: "round",
                                    stroke_linejoin: "round",
                                    stroke_width: "2",
                                    d: "M11.983 13.945a1.962 1.962 0 1 0 0-3.925 1.962 1.962 0 0 0 0 3.925zm7.436-2.12a6.994 6.994 0 0 0-.145-1.322l1.528-1.193a.511.511 0 0 0 .122-.645l-1.448-2.505a.515.515 0 0 0-.61-.236l-1.805.724a7.064 7.064 0 0 0-1.144-.663l-.273-1.92a.51.51 0 0 0-.504-.43h-2.897a.51.51 0 0 0-.504.43l-.273 1.92a7.034 7.034 0 0 0-1.144.663l-1.805-.724a.513.513 0 0 0-.61.236l-1.448 2.505a.51.51 0 0 0 .122.645l1.528 1.193c-.054.432-.09.872-.09 1.322 0 .45.036.89.09 1.322l-1.528 1.193a.511.511 0 0 0-.122.645l1.448 2.505c.132.23.41.33.61.236l1.805-.724c.36.27.75.498 1.144.663l.273 1.92c.045.262.27.43.504.43h2.897c.234 0 .459-.168.504-.43l.273-1.92a7.064 7.064 0 0 0 1.144-.663l1.805.724c.2.093.478-.006.61-.236l1.448-2.505a.511.511 0 0 0-.122-.645l-1.528-1.193c.108-.432.145-.872.145-1.322z",
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
