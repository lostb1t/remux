pub mod home;
pub use home::*;

// mod blog;
// pub use blog::Blog;

pub mod search;
pub use search::*;

pub mod settings;
pub use settings::Settings;

pub mod login;
pub mod media;
pub use login::*;

pub mod layout;
pub use layout::*;

use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, info};
use tracing_subscriber::field::debug;

#[component]
pub fn Loading(children: Element) -> Element {
    let mut loaded = use_signal(|| false);
    rsx! {
        div {
            onvisible: move |el| {
                loaded.set(true);
            },
            class: if !*loaded.read() { "sidebar-offset transition-opacity duration-2000 opacity-100 fixed inset-0 bg-neutral-900/100 z-40" } else { "sidebar-offset transition-opacity duration-2000 opacity-0 fixed inset-0 bg-neutral-900/100 z-40 pointer-events-none" },
        }
        {children}
    }
}
