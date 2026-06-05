use dioxus::prelude::*;

#[component]
pub fn FormGroup(label: String, children: Element) -> Element {
    rsx! {
        div { class: "form-group",
            label { class: "form-label", "{label}" }
            {children}
        }
    }
}

#[component]
pub fn ToggleRow(
    label: String,
    checked: bool,
    on_change: EventHandler<bool>,
) -> Element {
    rsx! {
        div { class: "toggle-row",
            span { class: "toggle-label", "{label}" }
            label { class: "toggle",
                input {
                    r#type: "checkbox",
                    checked,
                    oninput: move |e| on_change.call(e.checked()),
                }
                span { class: "toggle-track" }
            }
        }
    }
}

#[component]
pub fn FormActions(children: Element) -> Element {
    rsx! {
        div { class: "form-actions", {children} }
    }
}
