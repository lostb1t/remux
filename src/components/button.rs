use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Outline,
    Ghost,
    Link,
    Destructive,
}

impl Default for ButtonVariant {
    fn default() -> Self {
        Self::Primary
    }
}

/// Button size options
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonSize {
    Small,
    Medium,
    Large,
}

impl Default for ButtonSize {
    fn default() -> Self {
        Self::Medium
    }
}

#[derive(Clone, Props, PartialEq)]
pub struct ButtonProps {
    children: Element,

    #[props(default)]
    variant: ButtonVariant,

    #[props(default)]
    size: ButtonSize,

    //#[props(extends = GlobalAttributes)]
    #[props(extends = button, extends = GlobalAttributes)]
    attr: Vec<Attribute>,

    onclick: Option<EventHandler<MouseEvent>>,
}

#[component]
pub fn Button(props: ButtonProps) -> Element {
    let class = match props.variant {
        ButtonVariant::Primary => "px-4 py-2 rounded bg-white text-black border border-gray-300 hover:bg-gray-100 text-sm font-medium",
        ButtonVariant::Outline => "px-4 py-2 rounded border border-gray-300 hover:bg-gray-100 text-sm font-medium",
        _ => "px-4 py-2 rounded bg-white text-black border border-gray-300 hover:bg-gray-100 text-sm font-medium",
    };

    rsx!(
        button {
            class: class,
            onclick: move |event| if let Some(f) = props.onclick.as_ref() { f(event) },
            ..props.attr,
            {props.children}
        }
    )
}
