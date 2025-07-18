use dioxus::prelude::*;
use dioxus_primitives::switch;

#[derive(Props, PartialEq, Clone)]
pub struct SwitchProps {
    enabled: bool,
    on_toggle: EventHandler<bool>,

    #[props(optional)]
    class: Option<String>,
}

#[component]
pub fn Switch(props: SwitchProps) -> Element {
    let switch_class = if props.enabled {
        "relative inline-flex h-6 w-11 items-center rounded-full bg-green-600"
    } else {
        "relative inline-flex h-6 w-11 items-center rounded-full bg-zinc-700"
    };

    let thumb_class = if props.enabled {
        "inline-block h-4 w-4 transform rounded-full bg-white transition translate-x-6"
    } else {
        "inline-block h-4 w-4 transform rounded-full bg-white transition translate-x-1"
    };

    rsx! {
       switch::Switch {
            class: "{switch_class} {props.class.clone().unwrap_or_default()}",
            checked: props.enabled,
            on_checked_change: move |new_state| props.on_toggle.call(new_state),
            aria_label: "Toggle switch",
            switch::SwitchThumb {
                class: "{thumb_class}"
            }
        }
    }
}