use crate::js_bindings;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, error, info, trace, warn, Level};
use dioxus_use_js::EvalResultExt;
use rand::Rng;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json;
use std::rc::Rc;
use strum_macros::Display as EnumDisplay;
use strum_macros::EnumString;

fn generate_id() -> String {
    format!("{:06}", rand::thread_rng().gen::<u32>() % 1_000_000)
}

#[derive(EnumDisplay, EnumString, Serialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
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
            && self.scroll_direction == other.scroll_direction
            && self.index == other.index
    }
}

#[component]
pub fn PaginatedList<T, F>(props: PaginatedListProps<T, F>) -> Element
where
    T: Clone + PartialEq + 'static,
    F: Fn(&T) -> Element + Clone + 'static,
{
    let id = use_memo(move || format!("pl-{}", generate_id()));
    let event_id = use_memo(move || format!("{}_load_more", id()));
    let mut index = props.index;
    let mut last_index_set_by_scroll = use_signal(|| 0);
    let direction = props.scroll_direction;
    let trigger_offset = props.trigger_offset;
    let on_load_more = props.on_load_more.clone();

    use_effect(move || {
        let index = index.read();
        if *index != *last_index_set_by_scroll.peek() {
            let js = format!(
                r#"
            (() => {{
                let el = document.getElementById("{id}");
                if (!el) return;
                let child = el.children[{index}];
                if (child) child.scrollIntoView({{
                    behavior: "smooth",
                    inline: "start",
                    block: "nearest"
                }});
            }})();
            "#,
                id = id(),
                index = *index
            );
            let _ = document::eval(&js);
        }
    });

    let on_scroll = {
        let id = id.clone();
        //let event_id = event_id();
        move |_| async move {
            let scroll_info: js_bindings::ScrollInfo = js_bindings::getScrollInfo(id())
                .await
                .deserialize()
                .unwrap();
            //debug!(?s, "uhu");
            let scroll_pos = match direction {
                ScrollDirection::Horizontal => {
                    scroll_info.scroll_left as f32 + scroll_info.client_width as f32
                }
                ScrollDirection::Vertical => {
                    scroll_info.scroll_top as f32 + scroll_info.client_height as f32
                }
            };

            // if let Ok(html_el) = el.dyn_into::<web_sys::HtmlElement>() {
            let val = js_bindings::findLastPartiallyVisibleIndex(id(), direction)
                .await
                .unwrap();
            let i: usize = serde_json::from_value(val).unwrap();
            //if let Some(i) = find_last_partially_visible_index(&el, scroll_direction) {

            //     last_index_set_by_scroll.set(i);
            //     index.set(i);
            // }
            //debug!(?i, "index");
            if i != *index.peek() {
                last_index_set_by_scroll.set(i);
                index.set(i);
            }

            let max_scroll = match direction {
                ScrollDirection::Horizontal => scroll_info.scroll_width as f32,
                ScrollDirection::Vertical => scroll_info.scroll_height as f32,
            };
            // debug!(?scroll_pos, ?max_scroll, ?trigger_offset, "tracking scroll");
            if max_scroll - scroll_pos < trigger_offset {
                if let Some(cb) = &on_load_more {
                    cb.call(());
                }
            }
        }
    };

    let base_class = match direction {
        ScrollDirection::Vertical => {
            "flex flex-col overflow-y-auto no-scrollbar scroll-smooth gap-4"
        }
        ScrollDirection::Horizontal => {
            "flex overflow-x-auto no-scrollbar scroll-smooth snap-x snap-mandatory gap-4"
        }
    };

    let full_class = props
        .class
        .as_deref()
        .map(|c| format!("{base_class} {c}"))
        .unwrap_or_else(|| base_class.to_string());

    rsx! {
        div { id: "{id}", class: "{full_class}", onscroll: on_scroll,

            {props.items.iter().map(|item| rsx! {
                {(props.render_item)(item)}
            })}
        }
    }
}
