use dioxus::prelude::*;

#[component]
pub fn Modal(on_close: EventHandler, children: Element) -> Element {
    rsx! {
        div {
            class: "modal-backdrop",
            onclick: move |_| on_close.call(()),
            div {
                class: "modal",
                onclick: move |e| e.stop_propagation(),
                {children}
            }
        }
    }
}
