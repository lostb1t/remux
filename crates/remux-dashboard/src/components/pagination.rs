use dioxus::prelude::*;

fn selected_page(input: &str, total_pages: i64) -> Option<i64> {
    input
        .parse::<i64>()
        .ok()
        .map(|entered| {
            entered
                .saturating_sub(1)
                .clamp(0, total_pages - 1)
        })
}

#[component]
pub fn PaginationBar(
    page: Signal<i64>,
    total_pages: i64,
    #[props(default)] show_page_input: bool,
) -> Element {
    let page_v = *page.read();
    let mut page_input = use_signal(|| (page_v + 1).to_string());

    use_effect(move || {
        page_input.set(((*page.read()) + 1).to_string());
    });

    if total_pages <= 1 {
        return rsx! {};
    }

    let mut jump_to_page = move |raw: String| {
        if let Some(selected) = selected_page(&raw, total_pages) {
            page.set(selected);
            page_input.set((selected + 1).to_string());
        } else {
            page_input.set((*page.peek() + 1).to_string());
        }
    };

    rsx! {
        div { class: "pagination-bar",
            button {
                class: "btn btn-ghost",
                style: "height:28px;font-size:.75rem",
                disabled: page_v == 0,
                onclick: move |_| page.set((page_v - 1).max(0)),
                "‹ Prev"
            }
            if show_page_input {
                span { style: "display:flex;align-items:center;gap:6px;font-size:.8rem;opacity:.7",
                    "Page"
                    input {
                        class: "form-input",
                        r#type: "number",
                        min: "1",
                        max: "{total_pages}",
                        style: "height:28px;width:64px;padding:2px 6px;text-align:center",
                        value: "{page_input.read()}",
                        oninput: move |e| page_input.set(e.value()),
                        onchange: move |e| jump_to_page(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                let input = page_input.peek().clone();
                                jump_to_page(input);
                            }
                        },
                    }
                    "of {total_pages}"
                }
            } else {
                span { style: "font-size:.8rem;opacity:.7",
                    "Page {page_v + 1} of {total_pages}"
                }
            }
            button {
                class: "btn btn-ghost",
                style: "height:28px;font-size:.75rem",
                disabled: page_v + 1 >= total_pages,
                onclick: move |_| page.set(page_v + 1),
                "Next ›"
            }
        }

    }
}

#[cfg(test)]
mod tests {
    use super::selected_page;

    #[test]
    fn page_entry_is_one_based_and_clamped() {
        assert_eq!(selected_page("4", 10), Some(3));
        assert_eq!(selected_page("0", 10), Some(0));
        assert_eq!(selected_page("-9223372036854775808", 10), Some(0));
        assert_eq!(selected_page("11", 10), Some(9));
        assert_eq!(selected_page("", 10), None);
        assert_eq!(selected_page("999999999999999999999", 10), None);
    }
}
