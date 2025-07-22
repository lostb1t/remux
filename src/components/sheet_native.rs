use dioxus::prelude::*;
use std::time::Duration;
use tokio::time::sleep;
use tokio_with_wasm::alias as tokio;

#[component]
pub fn Sheet(open: Signal<bool>, title: Option<String>, children: Element) -> Element {
    let mut is_rendered = use_signal(|| false);
    let mut dragging = use_signal(|| false);
    let mut start_y = use_signal(|| 0.0f32);
    let mut delta_y = use_signal(|| 0.0f32);
    let mut should_animate_in = use_signal(|| false);

    use_effect(move || {
        if *open.read() {
            should_animate_in.set(true);
        } else {
            should_animate_in.set(false);
        }
    });

    use_effect(move || {
        if *open.read() {
            is_rendered.set(true);
        }
    });

    use_effect(move || {
        if !*open.read() {
            let mut is_rendered = is_rendered.clone();
            spawn(async move {
                sleep(Duration::from_millis(300)).await;
                is_rendered.set(false);
            });
        }
    });

    if !*is_rendered.read() {
        return rsx! {};
    }

    let on_pointer_down = move |evt: PointerEvent| {
        start_y.set(evt.data().client_coordinates().y as f32);
        delta_y.set(0.0);
        dragging.set(true);
    };

    let on_pointer_move = move |evt: PointerEvent| {
        let y = evt.data().client_coordinates().y as f32;
        if y > *start_y.read() {
            delta_y.set(y - *start_y.read());
        }
    };

    let on_pointer_up = move |_| {
        let delta = *delta_y.read();

        if delta > 225.0 {
            open.set(false);
        }

        delta_y.set(0.0);
        start_y.set(0.0);

        let mut dragging = dragging.clone();
        spawn(async move {
            sleep(Duration::from_millis(50)).await;
            dragging.set(false);
        });
    };

    let base_panel = "relative bg-white text-neutral-900 shadow-xl w-full \
                      sm:my-8 sm:w-full sm:max-w-lg rounded-t-2xl lg:rounded-lg";

    let panel_state = if *should_animate_in.read() {
        "translate-y-0 opacity-100 pointer-events-auto scale-100"
    } else {
        "translate-y-full opacity-0 pointer-events-none lg:scale-95"
    };

    let transform = if *dragging.read() {
        format!("transform: translateY({}px);", *delta_y.read())
    } else {
        "".to_string()
    };

    let transition_class = if *dragging.read() {
        ""
    } else {
        "transition-all duration-300 ease-in-out"
    };

    let backdrop_class = if *open.read() {
        "opacity-100 pointer-events-auto"
    } else {
        "opacity-0 pointer-events-none"
    };

    rsx! {
        div {
            class: "sidebar-offset fixed inset-0 z-50 flex items-end sm:items-center justify-center",
            role: "dialog",

            div {
                class: format!(
                    "absolute inset-0 bg-gray-500/50 backdrop-blur-sm transition-opacity duration-300 ease-in-out {backdrop_class}",
                ),
                onclick: move |_| open.set(false),
            }

            div {
                class: format!("{base_panel} {panel_state} {transition_class}"),
                style: "{transform}",
                onpointerdown: on_pointer_down,
                onpointermove: on_pointer_move,
                onpointerup: on_pointer_up,

                div { class: "min-h-[90vh] max-h-[90vh] overflow-y-auto pb-[env(safe-area-inset-bottom)]",
                    if let Some(title) = &title {
                        div { class: "absolute top-0 left-0 w-full h-12 flex items-center justify-center text-base font-semibold border-b border-gray-200 bg-white z-10",
                            "{title}"
                        }
                    }
                    div { class: "pt-12", {children} }
                }
            }
        }
    }
}