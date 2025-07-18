use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct ListProps {
    #[props(default)]
    title: Option<String>,

    #[props(default)]
    class: Option<String>,

    children: Element,
}

#[component]
pub fn List(props: ListProps) -> Element {
    let class = props.class.clone().unwrap_or_default();

    rsx! {
        div { class: "w-full flex flex-col gap-2 {class}",

            if let Some(title) = &props.title {
                h3 { class: "text-lg font-semibold px-4", "{title}" }
            }

            ul { class: "flex flex-col divide-y divide-black/10 rounded-xl overflow-hidden",
                {&props.children}
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ListItemProps {
    children: Element,

    #[props(default)]
    class: Option<String>,

    #[props(default)]
    onclick: Option<EventHandler<MouseEvent>>,
}

#[component]
pub fn ListItem(props: ListItemProps) -> Element {
    let class = props.class.clone().unwrap_or_default();

    rsx! {
        li { class: "px-4 py-3 cursor-pointer {class}",
            //   onclick: props.onclick.clone(),
            {&props.children}
        }
    }
}
