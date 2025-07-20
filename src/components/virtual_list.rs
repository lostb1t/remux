use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, info};
use tracing_subscriber::field::debug;
use std::rc::Rc;

use dioxus::web::WebEventExt;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::JsCast;
use web_sys::{ScrollBehavior, ScrollToOptions};

fn find_last_partially_visible_index(
    container: &web_sys::Element,
    direction: ScrollDirection,
) -> Option<usize> {
    let children = container.children();

    let (scroll_start, container_extent) = match direction {
        ScrollDirection::Horizontal => (container.scroll_left(), container.client_width()),
        ScrollDirection::Vertical => (container.scroll_top(), container.client_height()),
    };

    let scroll_end = scroll_start + container_extent;

    let mut last_visible = None;

    for i in 0..children.length() {
        let Some(child) = children.item(i) else {
            continue;
        };
        let Some(html_el) = child.dyn_ref::<web_sys::HtmlElement>() else { continue };
        // let child_el = child.dyn_ref::<web_sys::HtmlElement>().unwrap();

        let (item_start, item_end) = match direction {
            ScrollDirection::Horizontal => (
                html_el.offset_left(),
                html_el.offset_left() + html_el.offset_width(),
            ),
            ScrollDirection::Vertical => (
                html_el.offset_top(),
                html_el.offset_top() + html_el.offset_height(),
            ),
        };

        let is_visible = item_start < scroll_end && item_end > scroll_start;

        if is_visible {
            last_visible = Some(i as usize);
        }
    }

    last_visible
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum ScrollDirection {
    Vertical,
    #[default]
    Horizontal,
}

#[derive(Props, Clone)]
pub struct PaginatedListProps<T, F>
where
    T: Clone + PartialEq + 'static,
    F: Fn(&T) -> Element + Clone + 'static,
{
    pub items: Vec<T>,
    pub render_item: F,
    #[props(default)]
    pub on_load_more: Option<EventHandler<()>>,
    #[props(default = 1000.0)]
    pub trigger_offset: f32,
    #[props(default)]
    pub class: Option<String>,
    #[props(optional)]
    pub list_ref: Option<Signal<Option<MountedData>>>,
    #[props(default = ScrollDirection::Horizontal)]
    pub scroll_direction: ScrollDirection,
    #[props(default)]
    pub index: Signal<usize>,
}

impl<T: PartialEq + Clone + 'static, F: Fn(&T) -> Element + Clone + 'static> PartialEq
    for PaginatedListProps<T, F>
{
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items
            && self.on_load_more == other.on_load_more
            && self.trigger_offset == other.trigger_offset
            && self.class == other.class
            && self.list_ref == other.list_ref
            && self.scroll_direction == other.scroll_direction
            && self.index == other.index
    }
}

/// A vertical, lazily loading paginated list.
#[component]
pub fn PaginatedList<T, F>(props: PaginatedListProps<T, F>) -> Element
where
    T: Clone + PartialEq + 'static,
    F: Fn(&T) -> Element + Clone + 'static,
{
    let mut scroll_ref = use_signal(|| None as Option<Rc<MountedData>>);
    let on_load_more = props.on_load_more.clone();
    let trigger_offset = props.trigger_offset;
    let scroll_direction = props.scroll_direction;
    let mut index = props.index.clone();
    let mut last_index_set_by_scroll = use_signal(|| 0);

    // debug!(
    //     ?index,
    //     ?last_index_set_by_scroll,
    //     ?trigger_offset,
    //     "PaginatedList"
    // );

    use_effect(move || {
        let target = index.read();

        //if let Some(target) = target {
        if *target != *last_index_set_by_scroll.peek() {
            last_index_set_by_scroll.set(*target);

            if let Some(scroll_node) = scroll_ref() {
                // debug!(?target, "SCROLLINNNGGGGGG");
                let el = scroll_node.as_web_event();
                if let Some(child) = el.children().item(*target as u32) {
                    if let Some(child_el) = child.dyn_ref::<web_sys::HtmlElement>() {
                        let scroll_top = child_el.offset_top();
                        let scroll_left = child_el.offset_left();
                        // debug!(?scroll_top, "SCROLLING to item offset");
                        let mut options = ScrollToOptions::new();
                        match scroll_direction {
                            ScrollDirection::Vertical => options.top(scroll_top as f64),
                            ScrollDirection::Horizontal => options.left(scroll_left as f64),
                        };
                        options.behavior(ScrollBehavior::Smooth);
                        el.scroll_to_with_scroll_to_options(&options);
                    }
                }
            }
        }
    });

    let track_scroll = {
        let scroll_ref = scroll_ref.clone();
        let scroll_direction = scroll_direction;
        move || {
            if let Some(ref scroll_node) = scroll_ref() {
                let el = scroll_node.as_web_event();
                let scroll_ref_clone = scroll_ref.clone();
                let on_load_more = on_load_more.clone();
                let trigger_offset = trigger_offset;
                let scroll_direction = scroll_direction;
                let listener = Closure::<dyn FnMut(_)>::new(move |_event: web_sys::Event| {
                    if let Some(ref scroll_node) = scroll_ref_clone() {
                        let el = scroll_node.as_web_event();
                        let scroll_pos = match scroll_direction {
                            ScrollDirection::Horizontal => {
                                el.scroll_left() as f32 + el.client_width() as f32
                            }
                            ScrollDirection::Vertical => {
                                el.scroll_top() as f32 + el.client_height() as f32
                            }
                        };

                        // if let Ok(html_el) = el.dyn_into::<web_sys::HtmlElement>() {

                        if let Some(i) = find_last_partially_visible_index(&el, scroll_direction) {
                            // debug!(?i, "Found last partially visible index");
                            last_index_set_by_scroll.set(i);
                            index.set(i);
                        }

                        let max_scroll = match scroll_direction {
                            ScrollDirection::Horizontal => el.scroll_width() as f32,
                            ScrollDirection::Vertical => el.scroll_height() as f32,
                        };
                        // debug!(?scroll_pos, ?max_scroll, ?trigger_offset, "tracking scroll");
                        if max_scroll - scroll_pos < trigger_offset {
                            if let Some(cb) = &on_load_more {
                                cb.call(());
                            }
                        }
                    }
                });
                el.add_event_listener_with_callback("scroll", listener.as_ref().unchecked_ref())
                    .unwrap();
                listener.forget();
            }
        }
    };

    let base_class = match scroll_direction {
        ScrollDirection::Vertical => "flex flex-row flex-wrap items-start content-start overflow-y-auto no-scrollbar scroll-smooth gap-x-4 gap-y-4",
        // ScrollDirection::Horizontal => "pl-6 scroll-pl-6 flex overflow-x-auto no-scrollbar scroll-smooth gap-x-2 snap-x snap-mandatory",
        ScrollDirection::Horizontal => "flex overflow-x-auto no-scrollbar scroll-smooth snap-mandatory",
    };

    let class = if let Some(extra) = &props.class {
        format!("{} {}", base_class, extra)
    } else {
        base_class.to_string()
    };

    rsx! {
        div {
            class: "{class}",
            // style: "scrollbar-width: none; -ms-overflow-style: none;",
            style: "scrollbar-width: none;",
            onmounted: move |el| {
                scroll_ref.set(Some(el.data()));
                (track_scroll)();
            },

            {props.items.iter().map(|item| {
                rsx!(
                  {(props.render_item)(item)}
                )
            })}

        }
    }
}

#[derive(Props, Clone)]
pub struct CarouselListProps<T, F>
where
    T: Clone + PartialEq + 'static,
    F: Fn(&T) -> Element + Clone + 'static,
{
    pub items: Vec<T>,
    #[props(default)]
    pub index: Signal<usize>,
    pub render_item: F,
    #[props(default)]
    pub on_load_more: Option<EventHandler<()>>,
    #[props(default = "".to_string())]
    pub class: String,
}

impl<T: PartialEq + Clone + 'static, F: Fn(&T) -> Element + Clone + 'static> PartialEq
    for CarouselListProps<T, F>
{
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items && self.index == other.index
    }
}

#[component]
pub fn CarouselList<T, F>(props: CarouselListProps<T, F>) -> Element
where
    T: Clone + PartialEq + 'static,
    F: Fn(&T) -> Element + Clone + 'static,
{
    let render = props.render_item.clone();
    let items = props.items.clone();

    let base_class =
        "overflow-x-auto flex snap-x snap-mandatory scroll-smooth no-scrollbar".to_string();

    rsx! {
        PaginatedList {
            items: items.clone(),
            on_load_more: props.on_load_more.clone(),
            class: format!("{} {}", base_class, props.class),
            index: props.index.clone(),
            render_item: render
        }
    }
}
