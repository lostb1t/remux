use dioxus::prelude::*;

#[component]
pub fn PaginationBar(page: Signal<i64>, total_pages: i64) -> Element {
    let page_v = *page.read();

    if total_pages <= 1 {
        return rsx! {};
    }

    rsx! {
        div { class: "pagination-bar",
            button {
                class: "btn btn-ghost",
                style: "height:28px;font-size:.75rem",
                disabled: page_v == 0,
                onclick: move |_| page.set((page_v - 1).max(0)),
                "‹ Prev"
            }
            span { style: "font-size:.8rem;opacity:.7",
                "Page {page_v + 1} of {total_pages}"
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
