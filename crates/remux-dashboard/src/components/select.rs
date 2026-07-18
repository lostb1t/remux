use dioxus::prelude::*;
use std::rc::Rc;

/// One option in a [`Select`].
#[derive(Clone, PartialEq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

impl SelectOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// Inline style for the fixed-position menu, given the trigger's viewport rect
/// (`left`, `top`, `bottom`, `width` in px) and the viewport height (0 when
/// unknown). Anchors the menu below the trigger, flipping upward when there is
/// more room above it, and clamps the menu's max-height to the available space
/// so it never runs past the viewport edge.
fn menu_position_style(
    left: f64,
    top: f64,
    bottom: f64,
    width: f64,
    viewport_h: f64,
) -> String {
    const GAP: f64 = 6.0;
    const EDGE: f64 = 8.0;
    const MAX_H: f64 = 280.0;
    let below = (viewport_h - bottom - GAP - EDGE).max(0.0);
    let above = (top - GAP - EDGE).max(0.0);
    if viewport_h > 0.0 && above > below {
        let bottom_edge = viewport_h - top + GAP;
        format!(
            "position:fixed;top:auto;right:auto;left:{left:.0}px;width:{width:.0}px;bottom:{bottom_edge:.0}px;max-height:{:.0}px",
            above.min(MAX_H)
        )
    } else {
        let max_h = if viewport_h > 0.0 {
            below.min(MAX_H)
        } else {
            MAX_H
        };
        let top_edge = bottom + GAP;
        format!(
            "position:fixed;bottom:auto;right:auto;left:{left:.0}px;width:{width:.0}px;top:{top_edge:.0}px;max-height:{max_h:.0}px"
        )
    }
}

/// A modern dropdown that replaces the native `<select>`.
///
/// The native option list can't be themed with CSS, so this renders a styled
/// trigger plus a themed popover menu (selected option accented, click-outside
/// to close, Escape to close). Emits the chosen option's `value` via `on_change`.
///
/// The menu is measured from the trigger on open and rendered with
/// `position: fixed` in viewport coordinates: plain absolute positioning gets
/// clipped by `overflow: hidden` cards and the scrolling `.modal-body`, which
/// hid the menu whenever the control sat near the bottom of its container.
#[component]
pub fn Select(
    /// Currently-selected value.
    value: String,
    /// Options in display order.
    options: Vec<SelectOption>,
    /// Fired with the newly-selected value.
    on_change: EventHandler<String>,
    /// Shown when `value` matches no option.
    #[props(default)]
    placeholder: Option<String>,
    /// Extra class(es) for the wrapper (e.g. sizing).
    #[props(default)]
    class: Option<String>,
    /// When true the control is greyed out and can't be opened.
    #[props(default)]
    disabled: bool,
) -> Element {
    let mut open = use_signal(|| false);
    // Menu geometry measured from the trigger each time the menu opens. `None`
    // (or a failed measurement) falls back to the stylesheet's absolute position.
    let mut menu_style: Signal<Option<String>> = use_signal(|| None);
    let mut trigger_el: Signal<Option<Rc<MountedData>>> = use_signal(|| None);

    let current_label = options
        .iter()
        .find(|o| o.value == value)
        .map(|o| {
            o.label
                .clone()
        })
        .or_else(|| placeholder.clone())
        .unwrap_or_default();
    let has_value = options
        .iter()
        .any(|o| o.value == value);

    let wrapper_class = match &class {
        Some(c) => format!("cselect {c}"),
        None => "cselect".to_string(),
    };

    rsx! {
        div {
            class: "{wrapper_class}",
            // Escape closes the menu; handled on the wrapper so it works from
            // both the trigger and the option buttons. Stopped from bubbling so
            // it doesn't also dismiss an enclosing Modal.
            onkeydown: move |e| {
                if e.key() == Key::Escape && *open.read() {
                    e.stop_propagation();
                    open.set(false);
                }
            },
            button {
                r#type: "button",
                disabled,
                class: if *open.read() { "cselect-trigger cselect-trigger--open" } else { "cselect-trigger" },
                onmounted: move |e| trigger_el.set(Some(e.data())),
                onclick: move |_| {
                    if disabled { return; }
                    if *open.read() {
                        open.set(false);
                        return;
                    }
                    // Measure the trigger's viewport rect, then open the menu
                    // pinned to it (see the component doc comment).
                    let el = trigger_el.read().clone();
                    spawn(async move {
                        if let Some(el) = el {
                            if let Ok(rect) = el.get_client_rect().await {
                                let viewport_h = web_sys::window()
                                    .and_then(|w| w.inner_height().ok())
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.0);
                                menu_style.set(Some(menu_position_style(
                                    rect.min_x(),
                                    rect.min_y(),
                                    rect.max_y(),
                                    rect.width(),
                                    viewport_h,
                                )));
                            }
                        }
                        open.set(true);
                    });
                },
                span {
                    class: if has_value { "cselect-value" } else { "cselect-value cselect-placeholder" },
                    "{current_label}"
                }
                svg {
                    class: "cselect-chevron",
                    width: "16",
                    height: "16",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    polyline { points: "6 9 12 15 18 9" }
                }
            }
            if *open.read() {
                div {
                    class: "cselect-backdrop",
                    onclick: move |_| open.set(false),
                }
                div {
                    class: "cselect-menu",
                    style: if let Some(s) = menu_style.read().as_ref() { "{s}" } else { "" },
                    for opt in options.iter().cloned() {
                        {
                            let selected = opt.value == value;
                            let v = opt.value.clone();
                            rsx! {
                                button {
                                    r#type: "button",
                                    key: "{opt.value}",
                                    class: if selected { "cselect-option cselect-option--selected" } else { "cselect-option" },
                                    onclick: move |_| {
                                        on_change.call(v.clone());
                                        open.set(false);
                                    },
                                    span { "{opt.label}" }
                                    if selected {
                                        svg {
                                            class: "cselect-check",
                                            width: "15",
                                            height: "15",
                                            view_box: "0 0 24 24",
                                            fill: "none",
                                            stroke: "currentColor",
                                            stroke_width: "2.5",
                                            stroke_linecap: "round",
                                            stroke_linejoin: "round",
                                            polyline { points: "20 6 9 17 4 12" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mid-viewport trigger: menu opens below with the full max height.
    #[test]
    fn opens_below_with_full_height() {
        let style = menu_position_style(100.0, 260.0, 300.0, 220.0, 900.0);
        assert_eq!(
            style,
            "position:fixed;bottom:auto;right:auto;left:100px;width:220px;top:306px;max-height:280px"
        );
    }

    /// Trigger near the viewport bottom: menu flips up and is anchored to the
    /// trigger's top edge.
    #[test]
    fn flips_up_when_more_room_above() {
        let style = menu_position_style(100.0, 800.0, 840.0, 220.0, 900.0);
        assert_eq!(
            style,
            "position:fixed;top:auto;right:auto;left:100px;width:220px;bottom:106px;max-height:280px"
        );
    }

    /// Little room in either direction: clamp max-height to the larger side.
    #[test]
    fn clamps_max_height_to_available_space() {
        let style = menu_position_style(100.0, 600.0, 640.0, 220.0, 700.0);
        // below = 700-640-14 = 46, above = 600-14 = 586 → opens upward, clamped to 280.
        assert!(style.contains("bottom:106px"));
        assert!(style.contains("max-height:280px"));
        // And when below wins but is tight:
        let style = menu_position_style(100.0, 20.0, 640.0, 220.0, 700.0);
        assert!(style.contains("top:646px"));
        assert!(style.contains("max-height:46px"));
    }

    /// Unknown viewport height: fall back to opening below at full height.
    #[test]
    fn unknown_viewport_opens_below() {
        let style = menu_position_style(100.0, 260.0, 300.0, 220.0, 0.0);
        assert!(style.contains("top:306px"));
        assert!(style.contains("max-height:280px"));
    }
}
