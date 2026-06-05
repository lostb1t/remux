use dioxus::prelude::*;

#[component]
pub fn Card(
    title: String,
    #[props(default)] action: Option<Element>,
    #[props(default)] tight: bool,
    children: Element,
) -> Element {
    let body_class = if tight {
        "card-body tight"
    } else {
        "card-body"
    };
    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "{title}" }
                if let Some(action) = action {
                    {action}
                }
            }
            div { class: body_class,
                {children}
            }
        }
    }
}
