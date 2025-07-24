use crate::js_bindings;
use crate::utils;
use dioxus::events::TouchEvent;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, info};
use dioxus_use_js::EvalResultExt;
use std::ops::Deref;
use std::rc::Rc;
use std::time::Duration;
use tokio::time::sleep;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias as tokio;

/// Component that acts as an bottom sheet (mobile style) on small screens. And as a modal on larger
#[component]
pub fn Sheet(open: Signal<bool>, title: Option<String>, children: Element) -> Element {
    let mut is_rendered = use_signal(|| false);
    let mut dragging = use_signal(|| false);
    let mut start_y = use_signal(|| 0.0f32);
    let mut delta_y = use_signal(|| 0.0f32);
    let mut should_animate_in = use_signal(|| false);
    let mut content_ref = use_signal(|| None as Option<Rc<MountedData>>);
    let id = use_memo(move || format!("sheet-{}", utils::generate_id()));

    use_effect(move || {
        if *open.read() {
            let mut signal = should_animate_in.clone();
            spawn(async move {
                sleep(Duration::from_millis(10)).await;
                signal.set(true);
            });
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

    let on_touch_start = move |evt: TouchEvent| {
        if let Some(touch) = evt.touches().get(0) {
            start_y.set(touch.client_coordinates().y as f32);
            delta_y.set(0.0);
            dragging.set(true);
        }
    };

    let on_touch_move_old = move |evt: TouchEvent| {
        let read = content_ref.peek();

        async move {
            //let wut = read.as_ref().map(|el| el).unwrap();
            let scroll_info: js_bindings::ScrollInfo = js_bindings::getScrollInfo(id())
                .await
                .deserialize()
                .unwrap();
            let top = scroll_info.scroll_top;
            //debug!("on_touch_move_new: top={}", top);

            // only start dragging when pulling down and inner content is at the top
            if let Some(touch) = evt.touches().get(0) {
                let y = touch.client_coordinates().y as f32;
                if y > *start_y.peek() && top <= 0.0 {
                    evt.prevent_default();
                    delta_y.set(y - *start_y.peek());
                }
            }
        }
    };

    let on_touch_move = move |evt: TouchEvent| {
        // extract the y-position *synchronously* so we can compare
        if let Some(touch) = evt.touches().get(0) {
            let y = touch.client_coordinates().y as f32;
            let start = *start_y.peek();
            let id = id(); // clone into async below

            let should_prevent = y > start;

            if should_prevent {
                evt.prevent_default(); // <- needs to stay sync
            }

            // async part for JS scroll check
            spawn({
                let mut delta_y = delta_y.clone();
                async move {
                    let val = js_bindings::getScrollInfo(&id).await.unwrap();
                    let scroll_info: js_bindings::ScrollInfo = serde_json::from_value(val).unwrap();
                    if scroll_info.scroll_top <= 0.0 && should_prevent {
                        delta_y.set(y - start);
                    }
                }
            });
        }
    };

    let on_touch_end = move |_| {
        let delta = *delta_y.read();

        if delta > 225.0 {
            open.set(false);
            dragging.set(false);
            delta_y.set(0.0);
            start_y.set(0.0);
        } else {
            delta_y.set(0.0);
            start_y.set(0.0);

            let mut dragging = dragging.clone();
            spawn(async move {
                sleep(Duration::from_millis(50)).await;
                dragging.set(false);
            });
        }
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
                ontouchstart: on_touch_start,
                ontouchmove: on_touch_move,
                ontouchend: on_touch_end,

                div {
                    id: "{id}",
                    class: "min-h-[90vh] max-h-[90vh] overflow-y-auto pb-[env(safe-area-inset-bottom)]",
                    //  style: "max-height: calc(85vh - 24px);",
                    onmounted: move |el| {
                        content_ref.set(Some(el.data()));
                    },
                    if let Some(title) = &title {
                        // rsx! {
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
