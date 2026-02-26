use dioxus::prelude::*;


const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        h1 { "Remux Admin" }
        p { "Dashboard under construction." }
    }
}
