use crate::Route;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, info};

#[component]
pub fn Sidebar() -> Element {
    let navigator = use_navigator();
    let mut query = use_signal(|| "".to_string());

    rsx! {

        nav { class: "min-h-screen w-full bg-neutral-900/70 backdrop-blur-[80px] text-white flex flex-col items-start px-3 py-4 gap-3 text-sm",
            form {
                onsubmit: move |evt| {
                    if !query().is_empty() {
                        navigator
                            .push(Route::SearchView {
                                query: query().clone(),
                            });
                    }
                },
                input {
                    r#type: "text",
                    class: "w-full rounded-md px-3 py-1.5 bg-neutral-800 text-white placeholder:text-neutral-400 focus:outline-none focus:ring-2 focus:ring-blue-500",
                    value: query(),
                    placeholder: "Search",
                    oninput: move |e| query.set(e.value()),
                }
            }
            Link {
                to: Route::Home {},
                class: "flex items-center gap-3 px-2 py-1 rounded-md hover:bg-neutral-800 transition-colors w-full",
                svg {
                    xmlns: "http://www.w3.org/2000/svg",
                    class: "h-5 w-5 text-blue-500",
                    fill: "none",
                    view_box: "0 0 24 24",
                    stroke: "currentColor",
                    path {
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        stroke_width: "1",
                        d: "M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6",
                    }
                }
                span { class: "text-xs", "Home" }
            }

            Link {
                to: Route::Settings {},
                class: "flex items-center gap-3 px-2 py-1 rounded-md hover:bg-neutral-800 transition-colors w-full",
                svg {
                    xmlns: "http://www.w3.org/2000/svg",
                    class: "h-5 w-5 text-blue-500",
                    fill: "none",
                    view_box: "0 0 24 24",
                    stroke: "currentColor",
                    path {
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        stroke_width: "1",
                        d: "M11.983 13.945a1.962 1.962 0 1 0 0-3.925 1.962 1.962 0 0 0 0 3.925zm7.436-2.12a6.994 6.994 0 0 0-.145-1.322l1.528-1.193a.511.511 0 0 0 .122-.645l-1.448-2.505a.515.515 0 0 0-.61-.236l-1.805.724a7.064 7.064 0 0 0-1.144-.663l-.273-1.92a.51.51 0 0 0-.504-.43h-2.897a.51.51 0 0 0-.504.43l-.273 1.92a7.034 7.034 0 0 0-1.144.663l-1.805-.724a.513.513 0 0 0-.61.236l-1.448 2.505a.51.51 0 0 0 .122.645l1.528 1.193c-.054.432-.09.872-.09 1.322 0 .45.036.89.09 1.322l-1.528 1.193a.511.511 0 0 0-.122.645l1.448 2.505c.132.23.41.33.61.236l1.805-.724c.36.27.75.498 1.144.663l.273 1.92c.045.262.27.43.504.43h2.897c.234 0 .459-.168.504-.43l.273-1.92a7.064 7.064 0 0 0 1.144-.663l1.805.724c.2.093.478-.006.61-.236l1.448-2.505a.511.511 0 0 0-.122-.645l-1.528-1.193c.108-.432.145-.872.145-1.322z",
                    }
                }
                span { class: "text-xs", "Settings" }
            }
        }
    }
}
