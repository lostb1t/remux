use dioxus::prelude::*;
use dioxus_icons::lucide::{GripVertical, X};
use dioxus_primitives::drag_and_drop_list::{
    self, DragAndDropContext, DragAndDropDropIndicatorProps, DragAndDropItemContext,
    DragAndDropListItemProps, DragAndDropListItemsProps,
};

const AUTO_SCROLL_EDGE_PX: f64 = 96.0;
const AUTO_SCROLL_MAX_STEP_PX: f64 = 12.0;

#[css_module("/src/components/drag_and_drop_list/style.css")]
struct Styles;

#[derive(Props, Clone, PartialEq)]
pub struct DragAndDropListProps {
    /// Items (labels) to be rendered.
    pub items: Vec<Element>,

    /// Set if the list items should be removable
    #[props(default)]
    pub is_removable: bool,

    /// Accessible label for the list
    #[props(default)]
    pub aria_label: Option<String>,

    /// Additional attributes to apply to the list element.
    #[props(extends = GlobalAttributes)]
    pub attributes: Vec<Attribute>,

    /// The children of the list component.
    pub children: Element,

    /// Called when the items are reordered.
    #[props(default)]
    pub on_reorder: Option<EventHandler<Vec<String>>>,
}

// Monitor item re-ordering and execute some code when that happens
#[component]
fn ReorderObserver(on_reorder: EventHandler<Vec<String>>) -> Element {
    let items = drag_and_drop_list::use_drag_and_drop_list_items();
    let order: Vec<String> = items
        .into_iter()
        .map(|item| item.key)
        .collect();

    let initial_order = use_signal(|| order.clone());

    use_effect(use_reactive((&order,), move |(order,)| {
        if order != *initial_order.peek() {
            on_reorder.call(order);
        }
    }));

    rsx! {}
}

#[component]
pub fn DragAndDropList(props: DragAndDropListProps) -> Element {
    let is_removable = props.is_removable;
    let aria_label = props
        .aria_label
        .clone()
        .unwrap_or_else(|| "Sortable list".to_string());
    // Keep a stable key per item so Dioxus moves keyed siblings instead of
    // swapping content between list items during reorder.
    let items: Vec<Element> = props
        .items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let key = item
                .as_ref()
                .ok()
                .and_then(|v| {
                    v.key
                        .clone()
                })
                .unwrap_or_else(|| idx.to_string());
            rsx! {
                DragIcon { key: "{key}" }
                div { class: Styles::dx_item_body_div, {item} }
                if is_removable {
                    RemoveButton {}
                }
            }
        })
        .collect();

    rsx! {
        div {
            ondrag: auto_scroll_page,
            drag_and_drop_list::DragAndDropList {
                class: Styles::dx_dnd_list,
                items,
                aria_label: props.aria_label,
                attributes: props.attributes,
                drag_and_drop_list::DragAndDropInstructions {}
                DragAndDropListItems {
                    aria_label,
                }
                drag_and_drop_list::DragAndDropLiveRegion {}
                if let Some(on_reorder) = props.on_reorder {
                    ReorderObserver { on_reorder }
                }
                {props.children}
            }
        }
    }
}

fn edge_scroll_delta(cursor_y: f64, viewport_height: f64) -> f64 {
    if cursor_y < AUTO_SCROLL_EDGE_PX {
        -AUTO_SCROLL_MAX_STEP_PX * (1.0 - cursor_y.max(0.0) / AUTO_SCROLL_EDGE_PX)
    } else if cursor_y > viewport_height - AUTO_SCROLL_EDGE_PX {
        AUTO_SCROLL_MAX_STEP_PX
            * (1.0 - (viewport_height - cursor_y).max(0.0) / AUTO_SCROLL_EDGE_PX)
    } else {
        0.0
    }
}

#[component]
pub fn DragAndDropListItem(props: DragAndDropListItemProps) -> Element {
    rsx! {
        drag_and_drop_list::DragAndDropListItem {
            class: Styles::dx_dnd_list_item,
            index: props.index,
            // Forward the stable item key so the primitive tracks focus by
            // identity across reorders and removals instead of losing it.
            item_key: props.item_key.clone(),
            attributes: props.attributes,
            {props.children}
        }
    }
}

#[component]
pub fn DragAndDropListItems(props: DragAndDropListItemsProps) -> Element {
    rsx! {
        drag_and_drop_list::DragAndDropListItems {
            class: Styles::dx_dnd_list_ul,
            aria_label: props.aria_label,
            attributes: props.attributes,
            for item in drag_and_drop_list::use_drag_and_drop_list_items() {
                Fragment {
                    key: "{item.key}",
                    DragAndDropDropIndicator {
                        index: item.index,
                        position: "before",
                    }
                    DragAndDropListItem {
                        index: item.index,
                        item_key: item.key.clone(),
                        {item.children}
                    }
                    DragAndDropDropIndicator {
                        index: item.index,
                        position: "after",
                    }
                }
            }
        }
    }
}

fn auto_scroll_page(event: DragEvent) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(viewport_height) = window
        .inner_height()
        .ok()
        .and_then(|height| height.as_f64())
    else {
        return;
    };

    let cursor_y = event
        .client_coordinates()
        .y;
    let scroll_delta = edge_scroll_delta(cursor_y, viewport_height);
    if scroll_delta != 0.0 {
        window.scroll_by_with_x_and_y(0.0, scroll_delta);
    }
}

#[component]
pub fn DragAndDropDropIndicator(props: DragAndDropDropIndicatorProps) -> Element {
    rsx! {
        drag_and_drop_list::DragAndDropDropIndicator {
            class: Styles::dx_drop_indicator,
            index: props.index,
            position: props.position,
            attributes: props.attributes,
        }
    }
}

#[component]
fn DragIcon() -> Element {
    rsx! {
        GripVertical {
            class: Styles::dx_item_icon,
            "aria-hidden": "true",
            size: "16px",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolls_toward_viewport_edges() {
        assert_eq!(edge_scroll_delta(0.0, 800.0), -AUTO_SCROLL_MAX_STEP_PX);
        assert_eq!(edge_scroll_delta(400.0, 800.0), 0.0);
        assert_eq!(edge_scroll_delta(800.0, 800.0), AUTO_SCROLL_MAX_STEP_PX);
    }

    #[test]
    fn scroll_speed_increases_nearer_the_edge() {
        assert!(edge_scroll_delta(20.0, 800.0) < edge_scroll_delta(80.0, 800.0));
        assert!(edge_scroll_delta(780.0, 800.0) > edge_scroll_delta(720.0, 800.0));
    }
}

#[component]
pub fn RemoveButton(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let mut ctx: DragAndDropContext = use_context();
    let item_ctx: DragAndDropItemContext = use_context();
    let index = item_ctx.index();
    let label = format!("Remove item {}", index + 1);
    rsx! {
        button {
            class: Styles::dx_remove_button,
            r#type: "button",
            aria_label: "{label}",
            draggable: "false",
            onpointerdown: move |event| event.stop_propagation(),
            onmousedown: move |event| event.stop_propagation(),
            onmouseup: move |event| event.stop_propagation(),
            ondragstart: move |event| {
                event.prevent_default();
                event.stop_propagation();
            },
            onkeydown: move |event| event.stop_propagation(),
            onclick: move |event| {
                event.stop_propagation();
                ctx.remove(index);
            },
            ..attributes,
            {children}
            X { size: "14px" }
        }
    }
}
