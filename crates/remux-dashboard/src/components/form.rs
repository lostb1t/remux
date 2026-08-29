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
pub fn Switch(
    checked: bool,
    on_change: EventHandler<bool>,
    #[props(default = false)] disabled: bool,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            role: "switch",
            class: "switch",
            aria_checked: "{checked}",
            disabled,
            "data-state": if checked { "checked" } else { "unchecked" },
            onclick: move |_| on_change.call(!checked),
            span { class: "switch-thumb" }
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
            Switch { checked, on_change }
        }
    }
}

#[component]
pub fn FormActions(children: Element) -> Element {
    rsx! {
        div { class: "form-actions", {children} }
    }
}
