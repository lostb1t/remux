use dioxus::prelude::*;

#[component]
pub fn LoadingText() -> Element {
    rsx! {
        span { class: "loading-text", "Loading…" }
    }
}

#[component]
pub fn ErrorAlert(message: String) -> Element {
    rsx! {
        div { class: "alert-error", "{message}" }
    }
}

#[component]
pub fn SuccessAlert(message: String) -> Element {
    rsx! {
        div { class: "alert-success", "{message}" }
    }
}

#[component]
pub fn EmptyState(message: String) -> Element {
    rsx! {
        div { class: "empty-state", "{message}" }
    }
}
