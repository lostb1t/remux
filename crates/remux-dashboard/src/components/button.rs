use dioxus::prelude::*;

#[derive(Clone, PartialEq)]
pub enum ButtonVariant {
    Primary,
    Ghost,
    Secondary,
    Danger,
}

impl ButtonVariant {
    fn css_class(&self) -> &'static str {
        match self {
            ButtonVariant::Primary => "btn btn-primary",
            ButtonVariant::Ghost => "btn btn-ghost",
            ButtonVariant::Secondary => "btn btn-secondary",
            ButtonVariant::Danger => "btn btn-danger",
        }
    }
}

#[component]
pub fn Button(
    variant: ButtonVariant,
    #[props(default)] disabled: bool,
    #[props(default = "button".to_string())] r#type: String,
    #[props(default)] onclick: EventHandler<MouseEvent>,
    children: Element,
) -> Element {
    let class = variant.css_class();
    rsx! {
        button {
            r#type,
            class,
            disabled,
            onclick: move |e| onclick.call(e),
            {children}
        }
    }
}
