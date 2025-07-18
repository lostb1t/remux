use dioxus::prelude::*;
use dioxus_free_icons::IconShape;

#[derive(Props, PartialEq, Clone)]
pub struct ToggleIconProps<
    T: IconShape + Clone + PartialEq + 'static,
    U: IconShape + Clone + PartialEq + 'static,
> {
    #[props(default = Some(20))]
    pub width: Option<u32>,

    #[props(default = Some(20))]
    pub height: Option<u32>,

    pub fill: String,

    pub icon: T,
    pub icon_active: U,
    pub active: bool,

    #[props(optional)]
    pub onclick: Option<EventHandler<MouseEvent>>,
}

pub fn ToggleIcon<
    T: IconShape + Clone + PartialEq + 'static,
    U: IconShape + Clone + PartialEq + 'static,
>(
    props: ToggleIconProps<T, U>,
) -> Element {
    let icon_node = if props.active {
        rsx! {
            dioxus_free_icons::Icon {
                width: props.width,
                height: props.height,
                fill: &props.fill,
                icon: props.icon_active.clone(),
            }
        }
    } else {
        rsx! {
            dioxus_free_icons::Icon {
                width: props.width,
                height: props.height,
                fill: &props.fill,
                icon: props.icon.clone(),
            }
        }
    };

    rsx! {
        div {
            onclick: move |event| {
                if let Some(f) = props.onclick.as_ref() {
                    f(event)
                }
            },
            class: "inline-block cursor-pointer",
            {icon_node}
        }
    }
}
