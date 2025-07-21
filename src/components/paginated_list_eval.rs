use dioxus::prelude::*;
use std::rc::Rc;
use rand::Rng;

fn generate_id() -> String {
    format!("{:06}", rand::thread_rng().gen::<u32>() % 1_000_000)
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
    let index = props.index;
    let direction = props.scroll_direction;
    let trigger_offset = props.trigger_offset;

    use_effect(move || {
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
            index = *index.read()
        );
        let _ = document::eval(&js);
    });

    let on_scroll = {
        let id = id.clone();
        let event_id = event_id();
        move |_| {
            let js = format!(
                r#"
                (() => {{
                    let el = document.getElementById("{id}");
                    if (!el) return;
                    let scroll = el.scroll{dir} + el.client{dir_cap};
                    let max = el.scroll{dir_cap};
                    if (max - scroll < {trigger}) {{
                        window.__dioxus_emit("{event_id}");
                    }}
                }})();
                "#,
                id = id(),
                dir = if direction == ScrollDirection::Horizontal { "Left" } else { "Top" },
                dir_cap = if direction == ScrollDirection::Horizontal { "Width" } else { "Height" },
                trigger = trigger_offset,
                event_id = event_id
            );
            let _ = document::eval(&js);
        }
    };

    let base_class = match direction {
        ScrollDirection::Vertical => "flex flex-col overflow-y-auto no-scrollbar scroll-smooth gap-4",
        ScrollDirection::Horizontal => "flex overflow-x-auto no-scrollbar scroll-smooth snap-x snap-mandatory gap-4",
    };

    let full_class = props
        .class
        .as_deref()
        .map(|c| format!("{base_class} {c}"))
        .unwrap_or_else(|| base_class.to_string());

    rsx! {
        div {
            id: "{id}",
            class: "{full_class}",
            onscroll: on_scroll,
            
            {props.items.iter().map(|item| rsx! { {(props.render_item)(item)} })}
        }
    }
}
