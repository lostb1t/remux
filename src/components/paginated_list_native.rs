use dioxus::prelude::*;
use std::collections::HashMap;

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
    #[props(default)]
    pub index: Signal<usize>,
    pub render_item: F,
    #[props(default)]
    pub on_load_more: Option<EventHandler<()>>,
    #[props(default)]
    pub scroll_direction: ScrollDirection,
    #[props(default)]
    pub class: Option<String>,
}

impl<T, F> PartialEq for PaginatedListProps<T, F>
where
    T: Clone + PartialEq + 'static,
    F: Fn(&T) -> Element + Clone + 'static,
{
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items
            && self.index == other.index
            && self.on_load_more == other.on_load_more
            && self.scroll_direction == other.scroll_direction
            && self.class == other.class
        // render_item intentionally skipped
    }
}

#[component]
pub fn PaginatedList<T, F>(props: PaginatedListProps<T, F>) -> Element
where
    T: Clone + PartialEq + 'static,
    F: Fn(&T) -> Element + Clone + 'static,
{
    let mut start = use_signal(|| None as Option<(f64, f64)>);
    let mut has_loaded_more = use_signal(|| false);
    let mut index = props.index;
    let items = props.items.clone();
    let on_load_more = props.on_load_more.clone();
    let direction = props.scroll_direction;

    let transforms: HashMap<usize, String> = items.iter().enumerate().map(|(i, _)| {
        let offset = i as isize - *index.read() as isize;
        let transform = match direction {
            ScrollDirection::Horizontal => format!("translateX({}%)", offset * 100),
            ScrollDirection::Vertical => format!("translateY({}%)", offset * 100),
        };
        (i, transform)
    }).collect();

    let on_pointer_down = move |evt: PointerEvent| {
        let coords = evt.client_coordinates();
        start.set(Some((coords.x, coords.y)));
    };

    let on_pointer_up = move |evt: PointerEvent| {
    if let Some((start_x, start_y)) = start() {
        let end = evt.client_coordinates();
        let (delta_x, delta_y) = (end.x - start_x, end.y - start_y);

        let (forward, backward) = match direction {
            ScrollDirection::Horizontal => (delta_x < -50.0, delta_x > 50.0),
            ScrollDirection::Vertical => (delta_y < -50.0, delta_y > 50.0),
        };

        let current_index = *index.read();

        if forward && current_index < items.len() - 1 {
            let new_index = current_index + 1;
            index.set(new_index);

            if new_index >= items.len().saturating_sub(4) && !*has_loaded_more.read() {
                if let Some(cb) = &on_load_more {
                    cb.call(());
                    has_loaded_more.set(true);
                }
            }
        } else if backward && current_index > 0 {
            index.set(current_index - 1);
        }

        start.set(None);
    }
  };

    let base_class = "relative overflow-hidden w-full h-full";
    let full_class = props.class.clone().unwrap_or_default();

    rsx! {
        div {
            class: "{base_class} {full_class}",
            onpointerdown: on_pointer_down,
            onpointerup: on_pointer_up,

            {props.items.iter().enumerate().map(|(i, item)| {
                let transform = transforms.get(&i).unwrap_or(&"translateX(0%)".to_string()).clone();
                rsx! {
                    div {
                        key: "{i}",
                        class: "absolute top-0 left-0 w-full h-full transition-transform duration-300 ease-in-out",
                        style: "transform: {transform};",
                        {(props.render_item)(item)}
                    }
                }
            })}
        }
    }
}

