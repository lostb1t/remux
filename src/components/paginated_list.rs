use dioxus::prelude::*;
use std::rc::Rc;
use dioxus::web::WebEventExt;

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
    let scroll_data = use_signal(|| None as Option<Rc<MountedData>>);
    let last_index_by_scroll = use_signal(|| 0usize);
    let index = props.index;

    // Scroll to child on index change
    use_effect(move || {
        scroll_data.with(|opt| {
            if let Some(mounted) = opt {
                if let Some(el) = mounted.as_web_event() {
                    let children = el.children();
                    let target = *index.read();
                    if let Some(child) = children.get(target) {
                        child.scroll_into_view(props.scroll_direction);
                        last_index_by_scroll.set(target);
                    }
                }
            }
        });
    });

    let on_scroll = move |_| {
        scroll_data.with(|opt| {
            if let Some(mounted) = opt {
                if let Some(el) = mounted.as_web_event() {
                    let scroll_offset = match props.scroll_direction {
                        ScrollDirection::Horizontal => el.scroll_left() + el.client_width(),
                        ScrollDirection::Vertical => el.scroll_top() + el.client_height(),
                    };
                    let max_scroll = match props.scroll_direction {
                        ScrollDirection::Horizontal => el.scroll_width(),
                        ScrollDirection::Vertical => el.scroll_height(),
                    };

                    let scroll_offset_f32 = scroll_offset as f32;
                    let max_scroll_f32 = max_scroll as f32;

                    if max_scroll_f32 - scroll_offset_f32 < props.trigger_offset {
                        if let Some(cb) = &props.on_load_more {
                            cb.call(());
                        }
                    }

                    let mut last_visible = 0;
                    for (i, child) in el.children().iter().enumerate() {
                        let bounds = child.get_bounding_client_rect();
                        let (start, end) = match props.scroll_direction {
                            ScrollDirection::Horizontal => (bounds.left(), bounds.right()),
                            ScrollDirection::Vertical => (bounds.top(), bounds.bottom()),
                        };

                        if start < max_scroll_f32 as f64 && end > 0.0 {
                            last_visible = i;
                        }
                    }

                    last_index_by_scroll.set(last_visible);
                    index.set(last_visible);
                }
            }
        });
    };

    let base_class = match props.scroll_direction {
        ScrollDirection::Vertical => "flex flex-col overflow-y-auto no-scrollbar scroll-smooth gap-4",
        ScrollDirection::Horizontal => "flex overflow-x-auto no-scrollbar scroll-smooth snap-x snap-mandatory gap-4",
    };

    let full_class = match &props.class {
        Some(extra) => format!("{base_class} {extra}"),
        None => base_class.to_string(),
    };

    rsx! {
        div {
            class: "{full_class}",
            onmounted: move |el| {
                scroll_data.set(Some(el.data()));
            },
            onscroll: on_scroll,
            {props.items.iter().map(|item| rsx! { {(props.render_item)(item)} })}
        }
    }
}