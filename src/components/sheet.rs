use dioxus::events::TouchEvent;
use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use dioxus_logger::tracing::{debug, info};
use gloo_timers::future::sleep;
use std::ops::Deref;
use std::rc::Rc;
use std::time::Duration;
use tracing_subscriber::field::debug;
use wasm_bindgen::JsCast;
use web_sys::HtmlElement;


/// Component that acts as an bottom sheet (mobile style) on small screens. And as a modal on larger
#[component]
pub fn Sheet(open: Signal<bool>, title: Option<String>, children: Element) -> Element {
    let mut is_rendered = use_signal(|| false);
    let mut dragging = use_signal(|| false);
    let mut start_y = use_signal(|| 0.0f32);
    let mut delta_y = use_signal(|| 0.0f32);
    let mut should_animate_in = use_signal(|| false);
    let mut content_ref = use_signal(|| None as Option<Rc<MountedData>>);

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
        if let Some(touch) = get_first_touch(&evt) {
            start_y.set(touch.client_y() as f32);
            delta_y.set(0.0);
            dragging.set(true);
        }
    };

    let on_touch_move = move |evt: TouchEvent| {
        let read = content_ref.read();
        let wut = read.as_ref().map(|el| el).unwrap();
        let top = wut.as_web_event().scroll_top();
        debug!("on_touch_move_new: top={}", top);

        // only start dragging when pulling down and inner content is at the top
        if let Some(touch) = get_first_touch(&evt) {
            let y = touch.client_y() as f32;
            if y > *start_y.read() && top <= 0 {
                evt.prevent_default();
                delta_y.set(y - *start_y.read());
            }
        }
    };

    let on_touch_move_old = move |evt: TouchEvent| {
        //on_touch_move_new(evt.clone());
        evt.prevent_default();
        if let Some(touch) = get_first_touch(&evt) {
            let y = touch.client_y() as f32;
            delta_y.set(y - *start_y.read());
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

    fn get_first_touch(evt: &TouchEvent) -> Option<web_sys::Touch> {
        let raw = evt.as_web_event();
        let touch_event = raw.dyn_ref::<web_sys::TouchEvent>()?;
        touch_event
            .touches()
            .item(0)
            .or_else(|| touch_event.changed_touches().item(0))
    }

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
            class: "fixed inset-0 z-50 flex items-end sm:items-center justify-center",
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
