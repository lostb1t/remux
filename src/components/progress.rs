use dioxus::prelude::*;

#[derive(Props, PartialEq, Clone)]
pub struct ProgressBarProps {
    progress: u32,
    #[props(default = 100)]
    max: u32,
    #[props(optional)]
    class: Option<String>,
}

#[component]
pub fn ProgressBar(props: ProgressBarProps) -> Element {
    let max = props.max.max(1);
    let progress = props.progress.min(max);
    let percentage = (progress * 100 / max).min(100);

    rsx! {
        div { class: "w-full h-2 bg-white/50 rounded overflow-hidden {props.class.clone().unwrap_or_default()}",
            div {
                class: "h-full bg-green-500 transition-all",
                style: "width: {percentage}%;",
            }
        }
    }
}
