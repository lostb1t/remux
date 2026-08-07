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

#[component]
pub fn ConfirmDialog(
    message: String,
    on_confirm: EventHandler,
    on_cancel: EventHandler,
) -> Element {
    rsx! {
        div {
            class: "modal-backdrop",
            div {
                class: "modal",
                style: "max-width:420px;padding:24px",
                onclick: move |e| e.stop_propagation(),
                p { style: "margin:0 0 20px;font-size:.9rem", "{message}" }
                div { style: "display:flex;gap:8px;justify-content:flex-end",
                    button {
                        class: "btn btn-ghost",
                        style: "height:32px;font-size:.75rem",
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        style: "height:32px;font-size:.75rem;background:var(--error);border-color:var(--error)",
                        onclick: move |_| on_confirm.call(()),
                        "Confirm"
                    }
                }
            }
        }
    }
}
